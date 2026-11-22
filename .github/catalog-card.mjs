// Draws the library card the README shows: how many calibrations, and when the library
// last changed. Run by the Catalog workflow after the index is built:
//   node .github/catalog-card.mjs site/index.json site
// Writes library.zh.svg and library.en.svg next to the index.
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const [indexPath, outDir] = process.argv.slice(2)
const entries = JSON.parse(readFileSync(indexPath, 'utf8'))
const count = entries.length
// The newest calibration, not the build: a rating or a manual refresh is not new data.
const newest = entries.map((e) => e.createdAt).filter(Boolean).sort().at(-1)
// The project lives in UTC+8, and a date alone reads wrong a day either side of midnight.
const day = (iso) => new Date(new Date(iso).getTime() + 8 * 3600e3).toISOString().slice(0, 10)
const updated = newest ? day(newest) : '—'

const text = {
  zh: { title: '标定库', unit: '份光盘标定', updated: `更新于 ${updated}` },
  en: { title: 'Calibration library', unit: count === 1 ? 'disc calibration' : 'disc calibrations', updated: `Updated ${updated}` },
}

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;')

function card(t) {
  const digits = String(count).length
  // The unit starts after the number, which is set in a wider weight.
  const unitX = 92 + digits * 19 + 8
  return `<svg xmlns="http://www.w3.org/2000/svg" width="420" height="96" viewBox="0 0 420 96" role="img" aria-label="${esc(t.title)}: ${count} ${esc(t.unit)}, ${esc(t.updated)}">
  <style>
    .bg { fill: #ffffff; stroke: #e4e2ec; }
    .ink { fill: #1d1b24; }
    .muted { fill: #6e6a7c; }
    .ring { stroke: #9045f9; }
    .hub { fill: #9045f9; }
    text { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif; }
    @media (prefers-color-scheme: dark) {
      .bg { fill: #16151b; stroke: #2c2a35; }
      .ink { fill: #f2f0f7; }
      .muted { fill: #9a96a8; }
    }
  </style>
  <rect class="bg" x="0.5" y="0.5" width="419" height="95" rx="12"/>
  <g transform="translate(48 48)" fill="none">
    <circle class="ring" r="26" stroke-width="2"/>
    <circle class="ring" r="19" stroke-width="1" opacity="0.45"/>
    <circle class="ring" r="13" stroke-width="1" opacity="0.25"/>
    <circle class="hub" r="5" stroke="none"/>
  </g>
  <text class="muted" x="92" y="34" font-size="13">${esc(t.title)}</text>
  <text class="ink" x="92" y="68" font-size="32" font-weight="700">${count}</text>
  <text class="ink" x="${unitX}" y="67" font-size="14">${esc(t.unit)}</text>
  <text class="muted" x="400" y="67" font-size="13" text-anchor="end">${esc(t.updated)}</text>
</svg>
`
}

for (const lang of ['zh', 'en']) writeFileSync(join(outDir, `library.${lang}.svg`), card(text[lang]))
