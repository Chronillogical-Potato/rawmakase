# RAWmakase website

Source for [rawmakase.com](https://rawmakase.com), built with Hugo 0.150.1
(extended, for WebP resizing). Run these commands from the repository root:

```sh
hugo server --source website                     # http://localhost:1313
hugo --source website --minify --panicOnWarning   # website/public/
```

Download links follow the latest published GitHub release of `pch/rawmakase`,
read at build time, not the development version in `Cargo.toml`.
`params.version` in `hugo.toml` is the local-preview fallback if GitHub cannot
be reached. CI fails on that warning rather than publishing stale links.

## Deploy

The root [Website workflow](../.github/workflows/website.yml) builds website
pull requests without deploying. Changes to `website/**` or that workflow on
`main` build and deploy to GitHub Pages. It can also be run manually from `main`.
Other branches can be built manually but cannot deploy.

After publishing app packages, the release workflow calls Website directly.
That build checks out the current `main` website and refreshes its published
download links, including when repairing an older release.

Website-only changes skip desktop CI, packaging and CodeQL. Changes to the Website
workflow also run the lightweight workflow linter. Mixed website/application
changes retain the applicable application checks. Release-tag validation is
unchanged.

Page copy and features live in `content/_index.md`; the layout is
`layouts/index.html`. Generated output and Hugo's cache are ignored by Git.
