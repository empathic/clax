# The Clax quickstart

Clax's landing page: an interactive flow chart from signing in to a first
comment round trip (`index.html`), and the ideas that would make it better
(`ideas.html`, from the page database's `extensions` collection, with
`ideas-data.js` as the list shown without one).

`clax.json` is the page's capability declaration. The web build
(`web/scripts/build-quickstart.mjs`) copies `index.html`, `style.css`,
`icon.svg`, `clax.json` and `logos/` to `web/dist/quickstart`, which the
binary embeds; `ideas.html`, `ideas-data.js` and this README do not ship,
and a copy without ideas hides the "Make it better" tab. The page ships with
Clax and is seeded into new galleries without the ideas page; the
repository's published copy is the one with ideas. The page can also
be published as an artifact (`clax publish index.html --dir web/quickstart`);
`docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md` §9
describes shipping it with Clax.

The page shows the reel, `reel.mp4` with `reel-poster.jpg` as its poster,
at the top when those files are published beside it. They are build
artifacts of `web/reel` (its composer writes `clax-reel.mp4`), kept out of
the repository and out of the shipped copy, so a copy without them shows no
video. A viewer can hide the video; the page remembers that in
`localStorage` (`clax-quickstart-reel-hidden`) and offers it back.
