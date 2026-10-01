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
download links, including when repairing an older release. It does not need
`WEBSITE_DISPATCH_TOKEN` or a cross-repository event.

Website-only changes skip desktop CI, packaging and CodeQL. Changes to the Website
workflow also run the lightweight workflow linter. Mixed website/application
changes retain the applicable application checks. Release-tag validation is
unchanged.

CodeQL uses the root `codeql.yml` workflow instead of GitHub's automatic default
setup, which cannot apply these workflow trigger filters. It retains the same
five languages, default queries and weekly scans of the repository. Default
setup is disabled; do not re-enable it alongside this workflow, since it would
run unfiltered scans and reject the workflow's analysis uploads.

Page copy and features live in `content/_index.md`; the layout is
`layouts/index.html`. Generated output and Hugo's cache are ignored by Git.

## Hosting and migration record

The website was imported from `pch/rawmakase-website` at
`82968d56ad4612af2f6a3443bfee31c27318bf85`, preserving its original Git history
as a parent of the import commit. [PR #42](https://github.com/pch/rawmakase/pull/42)
was merged with that ancestry intact on 2026-10-01.

`rawmakase.com` is configured on `pch/rawmakase` with **GitHub Actions** as the
Pages source and **Enforce HTTPS** enabled. The certificate covers both
`rawmakase.com` and `www.rawmakase.com`; `www` redirects to the apex domain.
The existing DNS records still point to GitHub Pages. The `github-pages`
environment allows the `main` branch and `v*` tags, since releases also deploy
the website.

Before transferring the domain, the new deployment was verified against the
previous live site's HTML. After transfer, the page, images, download links,
HTTPS certificate and `www` redirect were checked again. The old repository's
Deploy workflow is disabled, its custom domain was removed, and its README
points here. That repository is archived and the obsolete
`WEBSITE_DISPATCH_TOKEN` secret has been removed from `pch/rawmakase`.

For an emergency hosting rollback, unarchive `pch/rawmakase-website`, remove
the domain from the main repository and restore it on the old repository.
Re-enable its Deploy workflow and deploy there, then verify HTTPS and the
published page. The old source and Pages site have been retained for this
purpose. App release publication will still call the main repository's website
workflow, so restore the main hosting configuration after resolving the problem.
