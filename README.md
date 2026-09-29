# rawmakase-website

Landing page for [RAWmakase](https://github.com/pch/rawmakase), built with Hugo (extended, for WebP resizing).

```sh
hugo server        # preview at http://localhost:1313
hugo --minify      # build into public/
```

When a release ships, bump `params.version` in `hugo.toml`; every download link follows it.
Page copy and the feature list live in `content/_index.md`; the layout is `layouts/index.html`.
