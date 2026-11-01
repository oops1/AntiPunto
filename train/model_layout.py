"""Модель раскладки: двунаправленный трансформер над окном клавиш."""

import torch
from torch import nn

from keys import VOCAB

W = 64


class LayoutNet(nn.Module):
    def __init__(self, d: int = 128, layers: int = 4, heads: int = 4, ff: int = 384, win: int = W):
        super().__init__()
        self.tok = nn.Embedding(VOCAB, d)
        self.lay = nn.Embedding(2, d)
        self.pos = nn.Parameter(torch.zeros(1, win, d))
        nn.init.normal_(self.pos, std=0.02)
        layer = nn.TransformerEncoderLayer(d, heads, ff, dropout=0.1, activation="gelu", batch_first=True, norm_first=True)
        self.enc = nn.TransformerEncoder(layer, layers, enable_nested_tensor=False)
        self.norm = nn.LayerNorm(d)
        self.head = nn.Linear(d, 1)

    def forward(self, tok: torch.Tensor, lay: torch.Tensor) -> torch.Tensor:
        # без маски паддинга: статичная форма для NPU
        x = self.tok(tok) + self.lay(lay) + self.pos
        return self.head(self.norm(self.enc(x))).squeeze(-1)
