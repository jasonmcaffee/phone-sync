# Vendored web fonts

The gallery is set in the same faces as `media.jasonmcaffee.com`, and they are compiled into the
server binary (`include_bytes!` in `handlers::gallery_font`) so the page makes **no third-party
request at runtime** — the same property the public site holds.

| File | Family / weight | Used for | Source |
|---|---|---|---|
| `general-sans-400.woff2` | General Sans 400 | body text | [Fontshare](https://api.fontshare.com/v2/css?f[]=general-sans@400,500) (Indian Type Foundry) |
| `general-sans-500.woff2` | General Sans 500 | emphasis | Fontshare |
| `martian-mono-400.woff2` | Martian Mono 400 | the edge print — every numeral, the rail and the month rules | [Google Fonts](https://fonts.googleapis.com/css2?family=Martian+Mono:wght@400), SIL OFL |
| `excon-500.woff2` | Excon 500 | the sign-in heading | Fontshare |

## Martian Mono must be the **latin** subset

Google Fonts serves one `@font-face` block per unicode-range subset, and **latin is not the first
one** — the order for Martian Mono is cyrillic-ext, cyrillic, latin-ext, latin.

`media-site/scripts/vendor-fonts.cjs` keeps "the first block of each family+weight, since the latin
subset covers this site", which for Google means it keeps **cyrillic-ext**: a 3.1 KB file with
essentially no Latin letters. Because the `@font-face` rules here declare no `unicode-range`, the
browser does not skip that file — it uses it for whatever glyphs it happens to contain and falls back
per glyph for the rest, so a capital `A` in the rail rendered in a completely different typeface from
the letters beside it. It is a silent defect: the type looks *nearly* right.

The file here is the latin subset (10.4 KB), fetched from the block whose `unicode-range` contains
`U+0000-00FF`:

```bash
node -e '
const fs = require("fs");
const UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
(async () => {
  const css = await (await fetch("https://fonts.googleapis.com/css2?family=Martian+Mono:wght@400&display=swap", { headers: { "User-Agent": UA } })).text();
  const latin = css.split("@font-face").slice(1).find((b) => /unicode-range:[^;]*U\+0000-00FF/.test(b));
  const url = latin.match(/url\(([^)]+)\)/)[1];
  fs.writeFileSync("martian-mono-400.woff2", Buffer.from(await (await fetch(url, { headers: { "User-Agent": UA } })).arrayBuffer()));
})();'
```

`media-site` has the same bad file and the same symptom on its own edge print. Fixing that is a
change to the public site and is left for a separate ticket.
