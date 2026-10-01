# rawmakase-website

Landing page for [RAWmakase](https://github.com/pch/rawmakase), built with Hugo (extended, for WebP resizing).

```sh
hugo server        # preview at http://localhost:1313
hugo --minify      # build into public/
```

Download links follow the latest GitHub release of pch/rawmakase, read at build time. `params.version` in `hugo.toml` is only the fallback if GitHub can't be reached.

## Deploy

`.github/workflows/deploy.yml` builds the site and publishes it to GitHub Pages on every push to `main`, once a day, and whenever pch/rawmakase sends a `rawmakase-release` dispatch after publishing a release.
Enable it under Settings → Pages → Source: GitHub Actions.
Page copy and the feature list live in `content/_index.md`; the layout is `layouts/index.html`.
