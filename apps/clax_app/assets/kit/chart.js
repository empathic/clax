// Clax kit charts: small SVG charts themed by the kit.css tokens.
//
// Every function returns markup (a string): put it in the page with
// `el.innerHTML = line(...)`, or in Preact with kit.js's
// `<${Markup} html=${line(...)} />`.
//
//   line({labels, series: [{name, values, color?}]}, {height, format, legend, smooth, dots})
//   area(data, opts)                 a line chart filled below
//   bar({labels, series}, {stacked, horizontal?: false, format, legend})
//   pie({items: [{label, value, color?}]}, {donut, center, centerSub, legend})
//   donut(data, opts)                pie with a hole
//   sparkline(values, {width, height, color, fill})
//
// A data object may give `values` instead of `series` for one series.

// Line and bar charts draw their plot in a W-wide box stretched to the
// chart's width (lines keep their stroke width), with the labels drawn over
// it at their own size: a chart stays its height and its text 11 px however
// wide it is.
const W = 340;
const PALETTE = [1, 2, 3, 4, 5, 6].map((i) => `var(--k-chart-${i})`);

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? v : 0);
const r1 = (v) => Math.round(v * 10) / 10;

/** A compact number: 1200 -> 1.2k, 3400000 -> 3.4M. */
export function compact(v) {
  const a = Math.abs(v);
  if (a >= 1e9) return `${r1(v / 1e9)}B`;
  if (a >= 1e6) return `${r1(v / 1e6)}M`;
  if (a >= 1e4) return `${Math.round(v / 1e3)}k`;
  if (a >= 1e3) return `${r1(v / 1e3)}k`;
  return String(Math.round(v * 100) / 100);
}

/** About `count` round ticks covering [min, max]. */
export function ticks(min, max, count = 4) {
  if (min === max) { max = min + 1; }
  const raw = (max - min) / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) || raw;
  const lo = Math.floor(min / step) * step;
  const hi = Math.ceil(max / step) * step;
  const out = [];
  for (let v = lo; v <= hi + step / 2; v += step) out.push(Math.round(v / step) * step);
  return out;
}

function seriesOf(data) {
  const series = data.series || [{ name: data.name || '', values: data.values || [] }];
  return series.map((s, i) => ({ ...s, values: (s.values || []).map(num), color: s.color || PALETTE[i % PALETTE.length] }));
}

function legendOf(items) {
  return `<div class="k-chart-legend">${items
    .map((it) => `<span><i style="background:${it.color}"></i>${esc(it.name ?? it.label)}</span>`)
    .join('')}</div>`;
}

/** The SVG path of `pts` as a monotone cubic curve (no overshoot). */
function curve(pts) {
  const n = pts.length;
  if (n < 3) return pts.map((p, i) => `${i ? 'L' : 'M'}${r1(p[0])},${r1(p[1])}`).join('');
  const d = [];
  const m = [];
  for (let i = 0; i < n - 1; i++) d.push((pts[i + 1][1] - pts[i][1]) / (pts[i + 1][0] - pts[i][0]));
  m.push(d[0]);
  for (let i = 1; i < n - 1; i++) m.push(d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2);
  m.push(d[n - 2]);
  for (let i = 0; i < n - 1; i++) {
    if (d[i] === 0) { m[i] = 0; m[i + 1] = 0; continue; }
    const a = m[i] / d[i];
    const b = m[i + 1] / d[i];
    const h = a * a + b * b;
    if (h > 9) { const t = 3 / Math.sqrt(h); m[i] = t * a * d[i]; m[i + 1] = t * b * d[i]; }
  }
  let p = `M${r1(pts[0][0])},${r1(pts[0][1])}`;
  for (let i = 0; i < n - 1; i++) {
    const dx = (pts[i + 1][0] - pts[i][0]) / 3;
    p += `C${r1(pts[i][0] + dx)},${r1(pts[i][1] + m[i] * dx)} ${r1(pts[i + 1][0] - dx)},${r1(pts[i + 1][1] - m[i + 1] * dx)} ${r1(pts[i + 1][0])},${r1(pts[i + 1][1])}`;
  }
  return p;
}

function frame(labels, lo, hi, height, format, padLeft) {
  const top = 10;
  const bottom = height - 22;
  const ts = ticks(lo, hi, 4);
  const min = ts[0];
  const max = ts[ts.length - 1];
  const y = (v) => bottom - ((v - min) / (max - min || 1)) * (bottom - top);
  let g = '';
  let text = '';
  for (const t of ts) {
    g += `<line class="k-grid-line" x1="${padLeft}" x2="${W}" y1="${r1(y(t))}" y2="${r1(y(t))}" vector-effect="non-scaling-stroke"/>`;
    text += `<text x="${pct(padLeft - 6)}" y="${r1(y(t)) + 3.5}" text-anchor="end">${esc(format(t))}</text>`;
  }
  const n = labels.length;
  const every = Math.max(1, Math.ceil(n / 6));
  return { y, top, bottom, min, max, grid: g, text, every };
}

/** An x in the plot's W-wide box as a share of the chart's width. */
const pct = (x) => `${r1((x / W) * 100)}%`;

function xLabels(labels, x, every, height) {
  return labels
    .map((l, i) => (i % every === 0 ? `<text x="${pct(x(i))}" y="${height - 6}" text-anchor="middle">${esc(l)}</text>` : ''))
    .join('');
}

/** A line or bar chart: `plot` in the stretched box, `over` (labels,
 * dots) drawn at the chart's own scale. */
function xyChart(label, height, defs, plot, over) {
  return `<svg class="k-chart k-chart-xy" width="100%" height="${height}" style="height:${height}px" role="img" aria-label="${esc(label)}">`
    + `<svg width="100%" height="${height}" viewBox="0 0 ${W} ${height}" preserveAspectRatio="none" overflow="visible"><defs>${defs}</defs>${plot}</svg>`
    + `${over}</svg>`;
}

function lineLike(data, opts, filled) {
  const { height = 180, format = compact, legend, smooth = true, dots, zero = filled } = opts || {};
  const series = seriesOf(data);
  const labels = data.labels || series[0].values.map((_, i) => String(i + 1));
  const all = series.flatMap((s) => s.values);
  if (!all.length) return '';
  let lo = Math.min(...all);
  const hi = Math.max(...all);
  if (zero || lo > 0 && lo < hi * 0.35) lo = Math.min(0, lo);
  const padLeft = 34;
  const f = frame(labels, lo, hi, height, format, padLeft);
  const n = labels.length;
  const x = (i) => (n === 1 ? (padLeft + W) / 2 : padLeft + 8 + (i * (W - padLeft - 16)) / (n - 1));
  let body = '';
  let defs = '';
  let over = '';
  series.forEach((s, si) => {
    const pts = s.values.map((v, i) => [x(i), f.y(v)]);
    const d = smooth ? curve(pts) : pts.map((p, i) => `${i ? 'L' : 'M'}${r1(p[0])},${r1(p[1])}`).join('');
    if (filled) {
      const id = `kg${Math.random().toString(36).slice(2, 8)}${si}`;
      defs += `<linearGradient id="${id}" x1="0" x2="0" y1="0" y2="1"><stop offset="0" style="stop-color:${s.color};stop-opacity:.32"/><stop offset="1" style="stop-color:${s.color};stop-opacity:0"/></linearGradient>`;
      body += `<path d="${d}L${r1(pts[pts.length - 1][0])},${f.bottom}L${r1(pts[0][0])},${f.bottom}Z" style="fill:url(#${id})" class="k-chart-fade"/>`;
    }
    // Drawn left to right by a clip that grows: a dash animation measures
    // the path after the stretch, so on a wide chart it stops short.
    const clip = `kc${Math.random().toString(36).slice(2, 8)}${si}`;
    defs += `<clipPath id="${clip}"><rect class="k-chart-reveal" x="0" y="-8" width="${W}" height="${height + 16}"/></clipPath>`;
    body += `<path d="${d}" clip-path="url(#${clip})" vector-effect="non-scaling-stroke" style="fill:none;stroke:${s.color};stroke-width:2.5;stroke-linecap:round;stroke-linejoin:round"/>`;
    if (dots || n <= 12) {
      const last = pts.length - 1;
      over += pts.map((p, i) => (dots || i === last)
        ? `<circle cx="${pct(p[0])}" cy="${r1(p[1])}" r="${i === last ? 4 : 3}" style="fill:var(--k-surface);stroke:${s.color};stroke-width:2.5"/>` : '').join('');
    }
  });
  const svg = xyChart(series.map((s) => s.name).filter(Boolean).join(', ') || 'chart', height, defs, f.grid + body, f.text + xLabels(labels, x, f.every, height) + over);
  return `<div class="k-chart-wrap">${svg}${legend ?? series.length > 1 ? legendOf(series) : ''}</div>`;
}

/** A line chart. */
export const line = (data, opts) => lineLike(data, opts, false);

/** A line chart filled with a gradient below each line. */
export const area = (data, opts) => lineLike(data, opts, true);

/** A bar chart: grouped series, or `stacked`. */
export function bar(data, opts) {
  const { height = 180, format = compact, legend, stacked } = opts || {};
  const series = seriesOf(data);
  const labels = data.labels || series[0].values.map((_, i) => String(i + 1));
  const n = labels.length;
  if (!n) return '';
  const totals = labels.map((_, i) => series.reduce((a, s) => a + Math.max(0, s.values[i] || 0), 0));
  const all = stacked ? totals : series.flatMap((s) => s.values);
  const lo = Math.min(0, ...all);
  const hi = Math.max(...all, 0);
  const padLeft = 34;
  const f = frame(labels, lo, hi, height, format, padLeft);
  const slot = (W - padLeft - 4) / n;
  const x = (i) => padLeft + 4 + slot * (i + 0.5);
  const groups = stacked ? 1 : series.length;
  const bw = Math.min(28, (slot * 0.66) / groups);
  const rad = Math.min(6, bw / 2);
  const rect = (x0, y0, y1, color, roundTop, delay) => {
    const h = Math.max(0, y1 - y0);
    if (h <= 0.5) return '';
    const r = roundTop ? Math.min(rad, h) : 0;
    const d = `M${r1(x0)},${r1(y1)}V${r1(y0 + r)}${r ? `Q${r1(x0)},${r1(y0)} ${r1(x0 + r)},${r1(y0)}H${r1(x0 + bw - r)}Q${r1(x0 + bw)},${r1(y0)} ${r1(x0 + bw)},${r1(y0 + r)}` : `H${r1(x0 + bw)}`}V${r1(y1)}Z`;
    return `<path d="${d}" class="k-chart-grow" style="fill:${color};animation-delay:${delay}ms"/>`;
  };
  let body = '';
  labels.forEach((_, i) => {
    if (stacked) {
      let acc = 0;
      series.forEach((s, si) => {
        const v = Math.max(0, s.values[i] || 0);
        body += rect(x(i) - bw / 2, f.y(acc + v), f.y(acc), s.color, si === series.length - 1 || !series.slice(si + 1).some((t) => t.values[i] > 0), i * 30);
        acc += v;
      });
    } else {
      series.forEach((s, si) => {
        const v = s.values[i] || 0;
        const x0 = x(i) - (bw * groups) / 2 + si * bw + (groups > 1 ? 1 : 0);
        body += rect(x0, f.y(Math.max(0, v)), f.y(Math.min(0, v)), s.color, true, i * 30);
      });
    }
  });
  const svg = xyChart('bar chart', height, '', f.grid + body, f.text + xLabels(labels, x, f.every, height));
  return `<div class="k-chart-wrap">${svg}${legend ?? series.length > 1 ? legendOf(series) : ''}</div>`;
}

/** A pie chart, or a donut with `donut: true` and an optional `center` text. */
export function pie(data, opts) {
  const { size = 180, donut, center, centerSub, legend = true } = opts || {};
  const items = (data.items || []).map((it, i) => ({ ...it, value: Math.max(0, num(it.value)), color: it.color || PALETTE[i % PALETTE.length] }));
  const total = items.reduce((a, it) => a + it.value, 0);
  const c = size / 2;
  const r = c - 4;
  const inner = donut ? r * 0.62 : 0;
  let a0 = -Math.PI / 2;
  let body = '';
  if (!total) {
    body = `<circle cx="${c}" cy="${c}" r="${r}" style="fill:var(--k-surface-3)"/>`;
  }
  for (const it of total ? items : []) {
    const a1 = a0 + (it.value / total) * Math.PI * 2;
    if (it.value === total) {
      body += `<circle cx="${c}" cy="${c}" r="${r}" style="fill:${it.color}"/>`;
    } else if (it.value > 0) {
      const large = a1 - a0 > Math.PI ? 1 : 0;
      const p = (a, rr) => `${r1(c + rr * Math.cos(a))},${r1(c + rr * Math.sin(a))}`;
      body += `<path d="M${p(a0, r)}A${r},${r} 0 ${large} 1 ${p(a1, r)}L${c},${c}Z" style="fill:${it.color};stroke:var(--k-surface);stroke-width:2"/>`;
    }
    a0 = a1;
  }
  if (donut) {
    body += `<circle cx="${c}" cy="${c}" r="${r1(inner)}" style="fill:var(--k-surface)"/>`;
    if (center != null) body += `<text class="k-chart-center" x="${c}" y="${c + (centerSub ? 2 : 7)}" text-anchor="middle">${esc(center)}</text>`;
    if (centerSub != null) body += `<text class="k-chart-center-sub" x="${c}" y="${c + 20}" text-anchor="middle">${esc(centerSub)}</text>`;
  }
  const svg = `<svg class="k-chart k-chart-pop" viewBox="0 0 ${size} ${size}" style="max-width:${size}px;margin:0 auto" role="img" aria-label="pie chart">${body}</svg>`;
  return `<div class="k-chart-wrap">${svg}${legend ? legendOf(items) : ''}</div>`;
}

/** A donut chart: `pie` with a hole. */
export const donut = (data, opts) => pie(data, { ...opts, donut: true });

/** A tiny trend line for inside a row or a stat. */
export function sparkline(values, opts) {
  const { width = 96, height = 28, color = 'var(--k-accent)', fill = true } = opts || {};
  const v = (values || []).map(num);
  if (v.length < 2) return '';
  const lo = Math.min(...v);
  const hi = Math.max(...v);
  const pts = v.map((y, i) => [2 + (i * (width - 4)) / (v.length - 1), height - 3 - ((y - lo) / (hi - lo || 1)) * (height - 6)]);
  const d = curve(pts);
  const id = `ks${Math.random().toString(36).slice(2, 8)}`;
  return `<svg class="k-chart" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" aria-hidden="true">${fill
    ? `<defs><linearGradient id="${id}" x1="0" x2="0" y1="0" y2="1"><stop offset="0" style="stop-color:${color};stop-opacity:.28"/><stop offset="1" style="stop-color:${color};stop-opacity:0"/></linearGradient></defs><path d="${d}L${width - 2},${height}L2,${height}Z" style="fill:url(#${id})"/>`
    : ''}<path d="${d}" style="fill:none;stroke:${color};stroke-width:2;stroke-linecap:round"/></svg>`;
}
