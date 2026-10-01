// The public stats page: this week's running total, weekly totals as a bar
// chart and the latest completed week's breakdowns. Self-contained, with no third-party requests.
import { DIMENSIONS } from "./dimensions";
import { MIN_GROUP, type Breakdown, type Stats, type Week } from "./publish";

const SOURCE = "https://github.com/pch/rawmakase/tree/main/stats";

export function renderPage({ thisWeek, weeks }: Stats): string {
  const latest = weeks.find((w) => w.total !== null);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>RAWmakase usage stats</title>
<meta name="description" content="Weekly counts of RAWmakase installations that opted in to anonymous reporting.">
<meta name="theme-color" content="#16191c">
<style>${STYLE}</style>
</head>
<body>
<main>
<header>
<p class="eyebrow"><a href="https://rawmakase.com">RAWmakase</a></p>
<h1>Usage stats</h1>
<p class="lead">Anonymous weekly counts from RAWmakase installations that opted in.</p>
</header>
${thisWeek ? soFar(thisWeek) : ""}
${weeks.length === 0 ? `<p class="empty">No completed weeks yet.</p>` : chart(weeks)}
${latest ? breakdowns(latest) : ""}
<section class="notes">
<h2>How these numbers are made</h2>
<ul>
<li>An opted-in RAWmakase sends at most one report per calendar week: app version, operating system and its major release, CPU architecture, build channel, graphics backend, and on Linux the distribution family and display server. No identifier, file, photo or setting is ever included.</li>
<li>Each field is added to its own weekly tally, and the report to a daily count, on arrival; the report itself is not stored. The service never reads or records IP addresses; Cloudflare, which hosts it, sees them as for any website.</li>
<li>This week's total counts reports through the end of yesterday (UTC) and updates daily. Breakdowns are shown only for completed weeks (ISO weeks, UTC). Groups of fewer than ${MIN_GROUP} installations are merged into "other", and weeks with fewer than ${MIN_GROUP} reports show no numbers.</li>
<li>The numbers are estimates of opted-in installations, not of people: one person may use several computers, and most installations never opt in.</li>
<li>Anyone can send a report, and there's no easy way to stop fake ones: an open-source app can't prove a report is genuine. Checks and rate limits keep out junk and floods from one address, but a determined person could still inflate the numbers.</li>
<li>Tallies are deleted after 24 months.</li>
</ul>
<p><a href="${SOURCE}">Source code</a></p>
</section>
</main>
</body>
</html>
`;
}

function soFar(week: NonNullable<Stats["thisWeek"]>): string {
  const total = week.total === null ? `Fewer than ${MIN_GROUP}` : String(week.total);
  return `<section class="so-far">
<h2>This week so far</h2>
<p><strong>${total}</strong> installations reported in ${week.week}, through ${week.through}.</p>
</section>`;
}

function chart(weeks: Week[]): string {
  const series = [...weeks].reverse();
  const max = Math.max(MIN_GROUP, ...series.map((w) => w.total ?? 0));
  const width = 720;
  const height = 220;
  const pad = { top: 16, right: 8, bottom: 28, left: 8 };
  const slot = (width - pad.left - pad.right) / series.length;
  const bar = Math.max(2, Math.min(28, slot * 0.7));
  const plot = height - pad.top - pad.bottom;
  const bars = series
    .map((w, i) => {
      const x = pad.left + i * slot + (slot - bar) / 2;
      const label = `${w.week}: ${w.total === null ? `fewer than ${MIN_GROUP}` : w.total}`;
      if (w.total === null) {
        const y = height - pad.bottom - 2;
        return `<rect class="low" x="${x.toFixed(1)}" y="${y}" width="${bar.toFixed(1)}" height="2"><title>${label}</title></rect>`;
      }
      const h = Math.max(2, (w.total / max) * plot);
      const y = height - pad.bottom - h;
      return `<rect x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${bar.toFixed(1)}" height="${h.toFixed(1)}" rx="2"><title>${label}</title></rect>`;
    })
    .join("");
  const first = series[0]!.week;
  const last = series[series.length - 1]!.week;
  return `<section>
<h2>Reporting installations per week</h2>
<figure>
<svg viewBox="0 0 ${width} ${height}" role="img" aria-label="Weekly reporting installations from ${first} to ${last}, peaking at ${max}">
<line x1="${pad.left}" x2="${width - pad.right}" y1="${height - pad.bottom}" y2="${height - pad.bottom}"/>
${bars}
<text x="${pad.left}" y="${height - 8}">${first}</text>
<text x="${width - pad.right}" y="${height - 8}" text-anchor="end">${last}</text>
</svg>
</figure>
</section>`;
}

function breakdowns(week: Week): string {
  const tables = DIMENSIONS.flatMap(({ name, title }) => {
    const breakdown = week.breakdowns![name];
    return breakdown ? [table(title, breakdown)] : [];
  });
  return `<section>
<h2>Week ${week.week}: ${week.total} installations</h2>
<div class="tables">
${tables.join("\n")}
</div>
</section>`;
}

function table(title: string, breakdown: Breakdown): string {
  const rows = breakdown.shown.map((g) => row(g.key, g.count));
  if (breakdown.other !== null) rows.push(row("other", breakdown.other, true));
  return `<table>
<thead><tr><th scope="col">${title}</th><th scope="col" class="n">Count</th></tr></thead>
<tbody>${rows.join("")}</tbody>
</table>`;
}

function row(key: string, count: number, other = false): string {
  return `<tr${other ? ` class="other"` : ""}><td>${escape(key)}</td><td class="n">${count}</td></tr>`;
}

function escape(value: string): string {
  return value.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

// The website's palette (website/assets/css/main.css), with system fonts so
// the page loads nothing from elsewhere.
const STYLE = `
:root { --ink: #16191c; --ink-2: #1e2226; --rice: #f2ede3; --rice-dim: #b9b3a8; --rice-faint: #7d7a74; --salmon: #eb7449; --line: rgba(242, 237, 227, 0.12); color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; background: var(--ink); color: var(--rice); font: 400 16px/1.6 ui-sans-serif, system-ui, sans-serif; -webkit-font-smoothing: antialiased; }
main { max-width: 760px; margin: 0 auto; padding: 48px 16px 64px; }
a { color: var(--salmon); }
.eyebrow { margin: 0; font-size: 14px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; }
.eyebrow a { color: var(--rice-dim); text-decoration: none; }
h1 { margin: 8px 0 12px; font-size: 40px; line-height: 1.1; letter-spacing: -0.02em; }
h2 { margin: 40px 0 12px; font-size: 18px; }
.lead { margin: 0; color: var(--rice-dim); font-size: 18px; }
.empty { margin-top: 32px; color: var(--rice-dim); }
figure { margin: 0; padding: 16px; border: 1px solid var(--line); border-radius: 12px; background: var(--ink-2); }
svg { display: block; width: 100%; height: auto; }
svg rect { fill: var(--salmon); }
svg rect.low { fill: var(--rice-faint); }
svg line { stroke: var(--line); }
svg text { fill: var(--rice-faint); font: 12px ui-monospace, monospace; }
.tables { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 16px; }
table { width: 100%; border-collapse: collapse; font-size: 15px; }
th, td { padding: 6px 0; border-bottom: 1px solid var(--line); text-align: left; }
th { color: var(--rice-dim); font-weight: 500; }
.n { text-align: right; font-variant-numeric: tabular-nums; }
tr.other td { color: var(--rice-dim); }
.so-far p { margin: 0; color: var(--rice-dim); }
.so-far strong { color: var(--rice); font-size: 32px; font-weight: 700; font-variant-numeric: tabular-nums; margin-right: 6px; }
.notes { margin-top: 48px; color: var(--rice-dim); font-size: 15px; }
.notes h2 { color: var(--rice); }
.notes ul { padding-left: 20px; }
`;
