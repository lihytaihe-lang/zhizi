"""顺手·知字: a small character-level causal LM that scores IME candidates by
P(candidate | preceding text). Kept deliberately plain (GPT-2 style blocks,
learned positions, tied embeddings) so it exports cleanly to Core ML / ONNX
for the IME later.
"""

import json
import math
from dataclasses import asdict, dataclass

import torch
import torch.nn as nn
import torch.nn.functional as F


@dataclass
class Config:
    vocab_size: int
    block_size: int = 128
    n_layer: int = 8
    n_head: int = 8
    d_model: int = 512
    dropout: float = 0.0


PRESETS = {
    "tiny": dict(n_layer=4, n_head=4, d_model=256),   # ~5M, for smoke tests
    "30m": dict(n_layer=8, n_head=8, d_model=512),     # ~29M
    "50m": dict(n_layer=10, n_head=10, d_model=640),   # ~54M
}


class Block(nn.Module):
    def __init__(self, c):
        super().__init__()
        self.ln1 = nn.LayerNorm(c.d_model)
        self.qkv = nn.Linear(c.d_model, 3 * c.d_model)
        self.proj = nn.Linear(c.d_model, c.d_model)
        self.ln2 = nn.LayerNorm(c.d_model)
        self.mlp = nn.Sequential(nn.Linear(c.d_model, 4 * c.d_model), nn.GELU(),
                                 nn.Linear(4 * c.d_model, c.d_model))
        self.n_head = c.n_head
        self.dropout = c.dropout

    def forward(self, x):
        b, t, d = x.shape
        q, k, v = self.qkv(self.ln1(x)).split(d, dim=2)
        q, k, v = (z.view(b, t, self.n_head, d // self.n_head).transpose(1, 2) for z in (q, k, v))
        y = F.scaled_dot_product_attention(q, k, v, is_causal=True,
                                           dropout_p=self.dropout if self.training else 0.0)
        x = x + self.proj(y.transpose(1, 2).reshape(b, t, d))
        return x + self.mlp(self.ln2(x))


class ZhiziLM(nn.Module):
    def __init__(self, c: Config):
        super().__init__()
        self.config = c
        self.tok = nn.Embedding(c.vocab_size, c.d_model)
        self.pos = nn.Embedding(c.block_size, c.d_model)
        self.blocks = nn.ModuleList(Block(c) for _ in range(c.n_layer))
        self.ln_f = nn.LayerNorm(c.d_model)
        self.head = nn.Linear(c.d_model, c.vocab_size, bias=False)
        self.head.weight = self.tok.weight
        self.apply(self._init)
        for name, p in self.named_parameters():
            if name.endswith("proj.weight") or name.endswith("mlp.2.weight"):
                nn.init.normal_(p, std=0.02 / math.sqrt(2 * c.n_layer))

    @staticmethod
    def _init(m):
        if isinstance(m, nn.Linear):
            nn.init.normal_(m.weight, std=0.02)
            if m.bias is not None:
                nn.init.zeros_(m.bias)
        elif isinstance(m, nn.Embedding):
            nn.init.normal_(m.weight, std=0.02)

    def forward(self, idx, targets=None):
        t = idx.shape[1]
        x = self.tok(idx) + self.pos(torch.arange(t, device=idx.device))
        for blk in self.blocks:
            x = blk(x)
        logits = self.head(self.ln_f(x))
        if targets is None:
            return logits
        loss = F.cross_entropy(logits.reshape(-1, logits.size(-1)), targets.reshape(-1), ignore_index=0)
        return logits, loss

    def n_params(self):
        return sum(p.numel() for p in self.parameters()) - self.pos.weight.numel()


class Vocab:
    def __init__(self, path=None, itos=None):
        self.itos = itos if itos is not None else json.load(open(path, encoding="utf-8"))
        self.stoi = {c: i for i, c in enumerate(self.itos)}
        self.pad, self.bos, self.unk = 0, 1, 2

    def encode(self, text):
        return [self.stoi.get(c, self.unk) for c in text]

    def __len__(self):
        return len(self.itos)


def save(path, model, vocab_path, step, extra=None):
    # The vocabulary travels inside the checkpoint so a model trained on a
    # rented GPU loads on the Mac without the original corpus paths.
    itos = json.load(open(vocab_path, encoding="utf-8"))
    torch.save({"config": asdict(model.config), "state": model.state_dict(), "vocab": vocab_path,
                "itos": itos, "step": step, **(extra or {})}, path)


def load(path, device="cpu"):
    ck = torch.load(path, map_location=device)
    model = ZhiziLM(Config(**ck["config"])).to(device)
    model.load_state_dict(ck["state"])
    model.eval()
    vocab = Vocab(itos=ck["itos"]) if "itos" in ck else Vocab(ck["vocab"])
    return model, vocab, ck
