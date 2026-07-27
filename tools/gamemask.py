"""实机截图里抠出宠物:取最大连通块。

**不能只用「与角落背景色的距离」**:好几张截图里宠物很小、背景是带花纹的卡片,
按距离判会把大片背景算进来(菊花梨/红绒十字/波波拉都栽在这儿,量出来的「实机颜色」
其实是背景色)。加两道:① 只保留最大连通块;② 面积占比 > 55% 视为抠图失败。
"""
import numpy as np
from PIL import Image


def game_mask(path, shrink=4):
    im = Image.open(path).convert('RGB')
    im = im.resize((im.width // shrink, im.height // shrink), Image.LANCZOS)
    a = np.array(im).astype(float)
    # 背景色取四角的中位
    h, w = a.shape[:2]
    corners = np.concatenate([a[:h//8, :w//8].reshape(-1, 3), a[:h//8, -w//8:].reshape(-1, 3),
                              a[-h//8:, :w//8].reshape(-1, 3), a[-h//8:, -w//8:].reshape(-1, 3)])
    bgc = np.median(corners, axis=0)
    m = np.linalg.norm(a - bgc, axis=2) > 45
    m[int(h * 0.86):, :] = False          # 底部水印
    # 最大连通块(4 邻域,迭代式洪水填充)
    lab = np.zeros(m.shape, np.int32); cur = 0; best = (0, None)
    import collections
    for sy in range(h):
        for sx in range(w):
            if m[sy, sx] and lab[sy, sx] == 0:
                cur += 1; q = collections.deque([(sy, sx)]); lab[sy, sx] = cur; n = 0
                while q:
                    y, x = q.popleft(); n += 1
                    for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                        ny, nx = y + dy, x + dx
                        if 0 <= ny < h and 0 <= nx < w and m[ny, nx] and lab[ny, nx] == 0:
                            lab[ny, nx] = cur; q.append((ny, nx))
                if n > best[0]: best = (n, cur)
    if best[1] is None: return None, None
    sel = lab == best[1]
    if sel.mean() > 0.55: return None, None      # 抠失败
    return a, sel
