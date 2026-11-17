import math

import torch


class ANEHiddenBody(torch.nn.Module):
    def __init__(self, lm, shape):
        super().__init__()
        if not isinstance(shape, (tuple, list)):
            raise ValueError("shape must be a (batch, width) pair of positive integers")
        if len(shape) != 2:
            raise ValueError("shape must be a (batch, width) pair of positive integers")
        batch, width = shape
        for dim in (batch, width):
            if isinstance(dim, bool) or not isinstance(dim, int) or dim <= 0:
                raise ValueError("shape must be a (batch, width) pair of positive integers")

        config = lm.config
        d_model = int(config.d_model)
        heads = int(config.n_head)
        if heads <= 0 or d_model % heads != 0:
            raise ValueError("d_model must be divisible by n_head")

        self.batch = int(batch)
        self.width = int(width)
        self.d_model = d_model
        self.heads = heads
        self.head_dim = d_model // heads
        self.scale = 1.0 / math.sqrt(self.head_dim)

        self.tok = lm.tok
        self.pos = lm.pos

        blocks = lm.blocks
        if not isinstance(blocks, torch.nn.ModuleList):
            blocks = torch.nn.ModuleList(list(blocks))
        self.blocks = blocks

        causal = torch.triu(torch.full((self.width, self.width), -1e4), diagonal=1)
        self.register_buffer("causal_mask", causal, persistent=False)

    def _conv_linear(self, x, linear):
        return torch.nn.functional.conv2d(x, linear.weight[:, :, None, None], bias=linear.bias)

    def _layer_norm(self, x, layer):
        mean = x.mean(dim=1, keepdim=True)
        centered = x - mean
        variance = (centered * centered).mean(dim=1, keepdim=True)
        return (
            centered * torch.rsqrt(variance + layer.eps) * layer.weight[None, :, None, None]
            + layer.bias[None, :, None, None]
        )

    def _mlp(self, x, mlp):
        for layer in mlp:
            if isinstance(layer, torch.nn.Linear):
                x = self._conv_linear(x, layer)
            else:
                x = torch.nn.functional.gelu(x)
        return x

    def forward(self, tokens, position):
        width = self.width
        heads = self.heads

        hidden = self.tok(tokens.long()) + self.pos.weight[:width]
        hidden = hidden.permute(0, 2, 1).unsqueeze(2)

        mask = (
            self.causal_mask.to(hidden.dtype).transpose(0, 1).reshape(1, width, 1, width)
        )

        for block in self.blocks:
            qkv = self._conv_linear(self._layer_norm(hidden, block.ln1), block.qkv)
            q, k, v = torch.chunk(qkv, 3, dim=1)
            q_heads = torch.chunk(q, heads, dim=1)
            k_heads = torch.chunk(k, heads, dim=1)
            v_heads = torch.chunk(v, heads, dim=1)

            contexts = []
            for h in range(heads):
                q_head = q_heads[h]
                k_permuted = k_heads[h].permute(0, 3, 2, 1)
                scores = torch.einsum("bchq,bkhc->bkhq", q_head, k_permuted) * self.scale
                scores = scores + mask
                attention = torch.softmax(scores, dim=1)
                contexts.append(torch.einsum("bchk,bkhq->bchq", v_heads[h], attention))

            hidden = hidden + self._conv_linear(torch.cat(contexts, dim=1), block.proj)
            mlp_in = self._layer_norm(hidden, block.ln2)
            hidden = hidden + self._mlp(mlp_in, block.mlp)

        selected = (hidden.squeeze(2) @ position.to(hidden.dtype).unsqueeze(-1)).squeeze(-1)
        return selected
