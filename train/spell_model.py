"""Ранжировщик кандидатов: оценка каждой строки «контекст | набранное | кандидат»."""

from pathlib import Path

import torch
from torch import nn

from spell_common import L, PAD, VOCAB


class SpellNet(nn.Module):
    def __init__(self, d: int = 128, layers: int = 3, heads: int = 4, ff: int = 384):
        super().__init__()
        self.tok = nn.Embedding(VOCAB, d)
        self.pos = nn.Parameter(torch.zeros(1, L, d))
        nn.init.normal_(self.pos, std=0.02)
        layer = nn.TransformerEncoderLayer(d, heads, ff, dropout=0.1, activation="gelu", batch_first=True, norm_first=True)
        self.enc = nn.TransformerEncoder(layer, layers, enable_nested_tensor=False)
        self.norm = nn.LayerNorm(d)
        self.head = nn.Linear(d, 1)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        """x: [N, L] → оценка [N]."""
        h = self.norm(self.enc(self.tok(x) + self.pos))
        keep = (x != PAD).unsqueeze(-1).float()
        pooled = (h * keep).sum(1) / keep.sum(1).clamp(min=1)
        return self.head(pooled).squeeze(-1)


def save_net(net: SpellNet, cfg: dict, path: Path) -> None:
    torch.save({"cfg": cfg, "state": net.state_dict()}, path)


def load_net(path: Path) -> SpellNet:
    """Чекпойнт с размером модели; старые (v1–v4) — голые веса маленькой модели."""
    ck = torch.load(path, map_location="cpu")
    cfg, state = (ck["cfg"], ck["state"]) if "cfg" in ck else ({}, ck)
    pos = state["pos"]
    if pos.shape[1] != L:
        # строка стала длиннее (правый контекст): старые позиции на месте, признаки — в конце
        old = pos.shape[1]
        new = torch.zeros(1, L, pos.shape[2])
        # новые позиции того же масштаба, что выученные: иначе их не отличить от беспозиционных
        torch.nn.init.normal_(new, std=float(pos[:, : old - 3].detach().std()))
        new[:, : old - 3] = pos[:, : old - 3]
        new[:, L - 3 :] = pos[:, old - 3 :]
        state = dict(state, pos=new)
    net = SpellNet(**cfg).eval()
    net.load_state_dict(state)
    return net
