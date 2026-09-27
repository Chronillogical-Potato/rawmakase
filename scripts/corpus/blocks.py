"""Block averages of a photo, as tests/color/private.rs computes them.

A photo is reduced to a grid 48 blocks across (rows chosen for square-ish blocks);
each block is the mean encoded-sRGB value as a 16-bit integer.
"""
COLUMNS = 48


def grid(width, height):
    rows = max(1, int(COLUMNS * height / width + 0.5))  # rounds half up, like Rust
    edge = lambda i, n, size: i * size // n  # noqa: E731
    for r in range(rows):
        for c in range(COLUMNS):
            x, y = edge(c, COLUMNS, width), edge(r, rows, height)
            yield x, y, edge(c + 1, COLUMNS, width) - x, edge(r + 1, rows, height) - y


def reduce(image):
    """`image`: H×W×3 array in 0–1. Returns [[r, g, b], ...] in grid order."""
    height, width = image.shape[:2]
    out = []
    for x, y, w, h in grid(width, height):
        mean = image[y:y + h, x:x + w].reshape(-1, 3).mean(0)
        out.append([int(min(max(v, 0.), 1.) * 65535 + 0.5) for v in mean])
    return out
