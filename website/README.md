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
five languages, default queries and weekly scans of the repository. Switch off
default setup when activating this workflow so it does not run a second set of
unfiltered scans or reject the workflow's analysis uploads.

Page copy and features live in `content/_index.md`; the layout is
`layouts/index.html`. Generated output and Hugo's cache are ignored by Git.

## GitHub Pages migration

The website was imported from `pch/rawmakase-website` at
`82968d56ad4612af2f6a3443bfee31c27318bf85`, preserving its original Git history
as a parent of the import commit. Merge this branch with a merge commit;
squashing or rebasing would discard that ancestry.

To transfer the live site after merging:

1. Enable GitHub Pages for `pch/rawmakase`, with **GitHub Actions** as its
   source. The `github-pages` environment must allow the `main` branch and
   `v*` tags, since the release workflow also deploys the website.
2. Disable the old repository's Deploy workflow. Remove its custom domain,
   then set `rawmakase.com` on the main repository's Pages settings. Verify
   the existing DNS records still point to GitHub Pages; the GitHub owner
   is unchanged. Keep the old source available for rollback.
3. Run **Website** from `main`. Wait for deployment and the domain's HTTPS
   certificate, enable **Enforce HTTPS**, then verify `rawmakase.com`, its
   images, and the download links against the latest published release.
4. Only after verification, replace the old repository's README with a link
   here and archive that repository. Remove the obsolete
   `WEBSITE_DISPATCH_TOKEN` secret from the main repository.

If the cutover fails, remove the domain from the main repository, restore it
on `pch/rawmakase-website`, re-enable its Deploy workflow and deploy there.
Do not delete the old Pages site or archive its repository before verification.
