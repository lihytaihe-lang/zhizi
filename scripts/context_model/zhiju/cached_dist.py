"""Bounded explicit attention caches for one pinyin-constrained search.

Weights, absolute positions and full-prefix cropping are unchanged. A caller
must prefill again when switching weights, dropping context or losing a parent.
"""
import torch
import torch.nn.functional as F

from export_dist import log_distribution


class CachedPrefill(torch.nn.Module):
    def __init__(self, lm, batch=8, width=40, capacity=40):
        super().__init__()
        self.lm, self.batch, self.width, self.capacity = lm, batch, width, capacity
        self.register_buffer("mask", torch.full((width, width), -1e4).triu(1))

    def forward(self, tokens, position):
        lm, b, t = self.lm, self.batch, self.width
        h, d = lm.config.n_head, lm.config.d_model
        z = lm.tok(tokens.long()) + lm.pos.weight[:t]
        keys, values = [], []
        for blk in lm.blocks:
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, t, h, d // h).transpose(1, 2)
            k = k.reshape(b, t, h, d // h).transpose(1, 2)
            v = v.reshape(b, t, h, d // h).transpose(1, 2)
            keys.append(F.pad(k, (0, 0, 0, self.capacity - t)))
            values.append(F.pad(v, (0, 0, 0, self.capacity - t)))
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5) + self.mask, dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, t, d))
            z = z + blk.mlp(blk.ln2(z))
        selected = (position.to(z.dtype)[:, None, :] @ z).squeeze(1)
        return log_distribution(lm.head(lm.ln_f(selected))), torch.stack(keys), torch.stack(values)


class CachedStep(torch.nn.Module):
    def __init__(self, lm, batch=8, capacity=40):
        super().__init__()
        self.lm, self.batch, self.capacity = lm, batch, capacity

    def forward(self, tokens, position, valid, keys, values):
        lm, b, capacity = self.lm, self.batch, self.capacity
        h, d = lm.config.n_head, lm.config.d_model
        absolute = (position.to(lm.pos.weight.dtype) @ lm.pos.weight[:capacity]).unsqueeze(1)
        z = lm.tok(tokens.long()) + absolute
        update = position[:, None, :, None].to(z.dtype)
        attention_mask = (1.0 - valid[:, None, None, :].to(z.dtype)) * -1e4
        new_keys, new_values = [], []
        for layer, blk in enumerate(lm.blocks):
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, 1, h, d // h).transpose(1, 2)
            k = k.reshape(b, 1, h, d // h).transpose(1, 2)
            v = v.reshape(b, 1, h, d // h).transpose(1, 2)
            k = keys[layer] * (1.0 - update) + k * update
            v = values[layer] * (1.0 - update) + v * update
            new_keys.append(k)
            new_values.append(v)
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5) + attention_mask, dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, 1, d))
            z = z + blk.mlp(blk.ln2(z))
        return log_distribution(lm.head(lm.ln_f(z[:, 0]))), torch.stack(new_keys), torch.stack(new_values)
