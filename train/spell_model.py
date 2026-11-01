"""Ранжировщик кандидатов: оценка каждой строки «контекст | набранное | кандидат»."""

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
