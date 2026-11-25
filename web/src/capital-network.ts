// «Структура групи капіталу» — the client of `GET /api/capital-network`.
//
// The payload follows the reference JSON: `metadata`, `nodes`, `links`,
// `styles`. The nodes carry normalised seed positions; a small force
// simulation (springs along the links, repulsion between the wallets, a
// collision pass) settles them into the picture of the reference — one flower
// with the hub in the middle and the group's wallets around it.
//
// The same layout is drawn twice: on a 2D canvas (the reference look — dot
// grid, glowing links with `screen` blending, nodes drawn hub > special >
// satellite) and in 3D with Three.js, where the 2D positions get a depth.

import { t } from "./i18n";
import { $, shortAddr, fmtUsd } from "./util";
import { escAttr } from "./ui";
import { getJSON } from "./api";
import { openGraphForWallet } from "./graph";

interface CnPosition { x: number; y: number }
interface CnNode {
  id: string;
  address: string;
  label?: string | null;
  type: string;
  role: string;
  relation?: string;
  color: string;
  stroke: string;
  radius: number;
  cluster: string;
  position: CnPosition;
  tokens?: string[];
  balanceUsd?: number;
  /** The place this wallet keeps returning to — its seat on the ring. */
  home?: { x: number; y: number };
  degree?: number;
  internalDegree?: number;
  share?: number;
  txWithHub?: number;
}
interface CnLink {
  source: string;
  target: string;
  type: string;
  weight?: number;
  color: string;
  width: number;
  opacity: number;
  tx?: number;
  usd?: number;
}
interface CnData {
  title?: string;
  metadata?: Record<string, unknown>;
  nodes: CnNode[];
  links: CnLink[];
  groups?: { id: string; label?: string; totalUsd?: number; size?: number; core?: boolean }[];
  styles?: {
    background?: { color?: string; grid_pattern?: { color?: string; spacing?: number; size?: number } };
    node?: { z_index?: string };
    link?: { mix_blend_mode?: string };
  };
  stats?: Record<string, number | string>;
  legend?: { color: string; key: string; label?: string }[];
}

/** A node of the simulation. */
interface SimNode {
  id: string;
  x: number;
  y: number;
  z: number;
  vx: number;
  vy: number;
  r: number;
  color: string;
  stroke: string;
  type: string;
  role: string;
  relation?: string;
  address: string;
  label?: string | null;
  tokens?: string[];
  balanceUsd?: number;
  /** The place this wallet keeps returning to — its seat on the ring. */
  home?: { x: number; y: number };
  degree?: number;
  internalDegree?: number;
  share?: number;
  fixed: boolean;
}
interface SimLink {
  s: number;
  t: number;
  weight: number;
  /** How strongly this link pulls (1 = full spring, 0 = no pull). */
  spring?: number;
  color: string;
  width: number;
  opacity: number;
  type: string;
  tx?: number;
}

/** Virtual canvas the simulation runs in (px). */
const SIM_W = 1000;
const SIM_H = 1000;
/** Distance each link type tries to keep (px). */
/**
 * The physics of the reference scheme (D3): the distance a link tries to keep
 * is `60 - weight * 5` — the heavier the link, the closer the wallets stay.
 * The scale turns the scheme's pixels into the simulation space.
 */
const LINK_BASE = 60;
const LINK_WEIGHT_STEP = 5;
const LINK_SCALE = 3.2;
const LINK_STRENGTH = 0.45;
const CHARGE = -400;
const COLLIDE_PAD = 2;
/**
 * The whole graph can be turned around the hub. The composition is now built
 * by the wide ring itself, so no tilt is applied (0 = straight). Positive angle
 * rotates "down" towards "down-left" (canvas Y grows down).
 */
const ROTATE_DEG = 0;
/**
 * The ring of neighbours is a wide ellipse around the hub (the backend seeds it
 * that way): the flower reads as a fan spread sideways, and the core keeps the
 * middle of it.
 */
/** How hard a neighbour is pulled back to its seat on the ellipse. */
const RING_PULL = 0.07;
/** From this many wallets the group is drawn as a dense ring, not as a fan. */
const DENSE_N = 400;
/** Cell of the spatial hash used by the repulsion and collision passes. */
let cnGridCell = 120;
/** How far the wallets push each other away (sim px). */
let cnRepulseR = 240;
/**
 * Everything in the simulation space is scaled by this factor — a group with
 * six thousand wallets needs a bigger space, otherwise the wallets sit on top
 * of each other. The picture on the screen stays the same.
 */
let cnScale = 1;
/** How hard the core and the pre-pump wallets hold their own places. */
const SEAT_PULL = 0.3;
/** How hard the second flower (the cohort) holds its own bunch. */
const COHORT_PULL = 0.12;
const CN_ROT_COS = Math.cos((ROTATE_DEG * Math.PI) / 180);
const CN_ROT_SIN = Math.sin((ROTATE_DEG * Math.PI) / 180);

let cnData: CnData | null = null;
let cnLoading = false;
let cnRaf = 0;
let cnSimRaf = 0;
let cnNodes: SimNode[] = [];
let cnLinks: SimLink[] = [];
let cnById = new Map<string, number>();
let cnAlpha = 1;
let cnView = { scale: 1, ox: 0, oy: 0 };
let cnHover = -1;
let cnPan: { x: number; y: number; ox: number; oy: number } | null = null;
let cnDrag: { idx: number; x: number; y: number } | null = null;
let cnCoreOnly = false;
/** The capital group the screen shows; empty — the largest one. */
let cnGroup = "";

export function openCapitalNetwork(): void {
  const modal = $("#capital-modal");
  if (!modal) return;
  modal.classList.remove("hidden");
  modal.classList.add("flex");
  cnView = { scale: 1, ox: 0, oy: 0 };
  void loadCapitalNetwork();
}

export function closeCapitalNetwork(): void {
  const modal = $("#capital-modal");
  if (modal) {
    modal.classList.add("hidden");
    modal.classList.remove("flex");
  }
  stop2D();
  const tip = $("#cn-tooltip");
  if (tip) tip.classList.add("hidden");
  const detail = $("#cn-detail");
  if (detail) detail.classList.add("translate-x-full");
}

export async function reloadCapitalNetwork(): Promise<void> {
  await loadCapitalNetwork();
}

async function loadCapitalNetwork(): Promise<void> {
  if (cnLoading) return;
  cnLoading = true;
  const status = $("#cn-status");
  if (status) status.textContent = t("cn_loading");
  try {
    const url = cnGroup ? `/api/capital-network?group=${encodeURIComponent(cnGroup)}` : "/api/capital-network";
    const d = await getJSON(url);
    cnData = d as CnData;
    buildModel(cnData);
    renderLegend(cnData);
    renderStats(cnData);
    renderGroupPicker(cnData);
    renderInfo(null);
    start2D();
  } catch {
    if (status) status.textContent = t("cn_error");
  } finally {
    cnLoading = false;
  }
}

/** Fill the group switcher with the capital groups of the payload. */
function renderGroupPicker(d: CnData): void {
  const sel = $("#cn-group") as HTMLSelectElement | null;
  if (!sel) return;
  const groups = d.groups || [];
  const current = String(d.metadata?.group || "");
  sel.innerHTML = "";
  for (const g of groups) {
    const opt = document.createElement("option");
    opt.value = String(g.id);
    const label = String(g.label || g.id);
    const usd = Number(g.totalUsd || 0);
    opt.textContent = `${label} · ${usd >= 1e9 ? (usd / 1e9).toFixed(2) + "B" : usd >= 1e6 ? (usd / 1e6).toFixed(1) + "M" : Math.round(usd)}`;
    sel.appendChild(opt);
  }
  if (current) sel.value = current;
}

/** The user picked another capital group — redraw the screen for it. */
export function setCapitalNetworkGroup(group: string): void {
  cnGroup = group || "";
  void loadCapitalNetwork();
}

/** Build the simulation model out of the payload. */
function buildModel(d: CnData): void {
  cnNodes = [];
  cnLinks = [];
  cnById = new Map();

  const nodes = d.nodes || [];
  // A group with a thousand wallets is drawn as a dense ring — the satellites
  // have to shrink, otherwise they merge into a blob.
  const dense = nodes.length > DENSE_N;
  // The bigger the group, the bigger the simulation space — the wallets keep
  // the same room between each other and the picture stays the same.
  cnScale = dense ? Math.sqrt(nodes.length / 150) : 1;
  cnGridCell = 120 * cnScale;
  cnRepulseR = 240 * cnScale;
  const hubSeed = nodes.find((n) => n.type === "hub" && n.role !== "cohort-hub");
  const hx = (hubSeed?.position?.x ?? 0.45) * SIM_W * cnScale;
  const hy = (hubSeed?.position?.y ?? 0.5) * SIM_H * cnScale;
  for (const n of nodes) {
    const i = cnNodes.length;
    cnById.set(n.id, i);
    // The whole graph is turned by ROTATE_DEG around the hub — the same way
    // the reference picture is composed (the core's fan goes to the side).
    const rawX = (n.position?.x ?? 0.5) * SIM_W * cnScale;
    const rawY = (n.position?.y ?? 0.5) * SIM_H * cnScale;
    const dx = rawX - hx;
    const dy = rawY - hy;
    const x = hx + dx * CN_ROT_COS - dy * CN_ROT_SIN;
    const y = hy + dx * CN_ROT_SIN + dy * CN_ROT_COS;
    const role = n.role || n.type;
    const z = 0;
    cnNodes.push({
      id: n.id,
      x,
      y,
      z,
      vx: 0,
      vy: 0,
      r: (n.radius || 4) * (dense && n.type === "satellite" ? 0.7 : 1.9) * cnScale,
      color: n.color || "#4A6FA5",
      stroke: n.stroke || "#3A5A8A",
      type: n.type,
      role,
      relation: n.relation,
      address: n.address,
      home: { x, y },
      label: n.label,
      tokens: n.tokens,
      balanceUsd: n.balanceUsd,
      degree: n.degree,
      internalDegree: n.internalDegree,
      share: n.share,
      fixed: n.type === "hub",
    });
  }
  for (const l of d.links || []) {
    const s = cnById.get(l.source);
    const t = cnById.get(l.target);
    if (s === undefined || t === undefined) continue;
    const a = cnNodes[s];
    const b = cnNodes[t];
    // The spokes of the group would squeeze the wide ring back into a circle,
    // so they barely pull — the ellipse keeps the fan wide.
    const spoke =
      (a.role === "hub" && b.role === "middle" && b.relation === "group") ||
      (b.role === "hub" && a.role === "middle" && a.relation === "group");
    cnLinks.push({
      s,
      t,
      weight: l.weight ?? 1,
      spring: spoke ? 0.06 : dense ? 0.25 : 1,
      color: l.color || "#4A6FA5",
      width: l.width || 1,
      opacity: l.opacity ?? 0.5,
      type: l.type || "standard",
      tx: l.tx,
    });
  }
  simulate();
}

/** The distance a link keeps: `60 - weight * 5`, as in the reference scheme. */
function linkDistance(l: SimLink): number {
  const w = l.weight ?? 1;
  return Math.max(12, LINK_BASE - w * LINK_WEIGHT_STEP) * LINK_SCALE * cnScale;
}

/**
 * The force simulation: springs along the links, repulsion between every pair
 * of wallets and a collision pass, so the wallets settle into a ring around
 * the hub instead of piling up on it.
 */
function simulate(): void {
  cnAlpha = 1;
  cancelAnimationFrame(cnSimRaf);
  // The seed picture is already in the frame; the final one is fitted again
  // when the simulation settles.
  fitView();
  // A group with a thousand wallets cannot be simulated for a thousand ticks —
  // the dense ring is settled much faster.
  const dense = cnNodes.length > DENSE_N;
  const maxTicks = cnNodes.length > 2000 ? 60 : dense ? 200 : 1200;
  let ticks = 0;
  const step = () => {
    for (let k = 0; k < 3; k++) tick();
    ticks += 3;
    if (cnAlpha > 0.02 && ticks < maxTicks) {
      cnSimRaf = requestAnimationFrame(step);
    } else {
      cnAlpha = 0.02;
      fitView();
    }
  };
  cnSimRaf = requestAnimationFrame(step);
}

/** The cell of the spatial hash a coordinate falls into. */
function cellOf(v: number): number {
  return Math.floor(v / cnGridCell) + 64;
}

/** The key of the spatial hash bucket a coordinate falls into. */
function cellKey(x: number, y: number): number {
  return cellOf(y) * 1024 + cellOf(x);
}

function tick(): void {
  const n = cnNodes.length;
  if (!n) return;

  // Springs along the links.
  for (const l of cnLinks) {
    const a = cnNodes[l.s];
    const b = cnNodes[l.t];
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const dist = Math.hypot(dx, dy) || 0.001;
    const target = linkDistance(l);
    const f = ((dist - target) / dist) * LINK_STRENGTH * cnScale * cnAlpha * (l.spring ?? 1);
    const fx = dx * f * 0.5;
    const fy = dy * f * 0.5;
    if (!a.fixed) {
      a.vx += fx;
      a.vy += fy;
    }
    if (!b.fixed) {
      b.vx -= fx;
      b.vy -= fy;
    }
  }

  // The wallets are bucketed into a spatial hash, so a group with a thousand
  // wallets does not cost a million checks per tick.
  const dense = n > DENSE_N;
  // In a dense ring the wallets keep their seats — the repulsion would only
  // blow the ring apart, and the collision would push it out of the screen.
  // Both passes are skipped there.
  if (dense) {
    for (const nd of cnNodes) {
      if (nd.fixed || !nd.home) continue;
      const seat =
        nd.role === "core" || nd.role === "prepump"
          ? SEAT_PULL * cnScale
          : nd.role === "middle" && nd.relation === "group"
            ? RING_PULL * cnScale
            : nd.role === "middle" && nd.relation === "asset"
              ? COHORT_PULL * cnScale
              : 0;
      if (!seat) continue;
      nd.vx += (nd.home.x - nd.x) * seat * cnAlpha;
      nd.vy += (nd.home.y - nd.y) * seat * cnAlpha;
    }
    for (const nd of cnNodes) {
      if (nd.fixed) {
        nd.vx = 0;
        nd.vy = 0;
        continue;
      }
      nd.vx *= 0.55;
      nd.vy *= 0.55;
      nd.x += nd.vx;
      nd.y += nd.vy;
      const pad = 40 * cnScale;
      nd.x = Math.max(pad, Math.min(SIM_W * cnScale - pad, nd.x));
      nd.y = Math.max(pad, Math.min(SIM_H * cnScale - pad, nd.y));
    }
    cnAlpha *= 0.992;
    return;
  }
  // In a dense ring the wallets keep their seats — the repulsion would only
  // blow the ring apart.
  const repulseK = dense ? 0 : 1;
  const collideK = dense ? 0 : 1;
  const grid = new Map<number, number[]>();
  for (let i = 0; i < n; i++) {
    const nd = cnNodes[i];
    const key = cellKey(nd.x, nd.y);
    const bucket = grid.get(key);
    if (bucket) bucket.push(i);
    else grid.set(key, [i]);
  }

  // Repulsion between the wallets.
  for (let i = 0; i < n; i++) {
    const a = cnNodes[i];
    const gx = cellOf(a.x);
    const gy = cellOf(a.y);
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const bucket = grid.get((gy + oy) * 1024 + (gx + ox));
        if (!bucket) continue;
        for (const j of bucket) {
          if (j <= i) continue;
          const b = cnNodes[j];
          const dx = b.x - a.x;
          const dy = b.y - a.y;
          const d2 = dx * dx + dy * dy;
          if (d2 > cnRepulseR * cnRepulseR) continue;
          if (d2 < 0.01) {
            // exactly on top of each other — push apart deterministically
            const ang = ((i * 37 + j * 17) % 360) * (Math.PI / 180);
            b.vx += Math.cos(ang) * 2;
            b.vy += Math.sin(ang) * 2;
            continue;
          }
          const d = Math.sqrt(d2);
          const f = ((CHARGE * cnScale * cnScale) / d) * cnAlpha * repulseK / 50;
          const fx = (dx / d) * f;
          const fy = (dy / d) * f;
          a.vx -= fx;
          a.vy -= fy;
          b.vx += fx;
          b.vy += fy;
        }
      }
    }
  }

  // Collision: nobody sits on anybody.
  for (let i = 0; i < n; i++) {
    const a = cnNodes[i];
    const gx = cellOf(a.x);
    const gy = cellOf(a.y);
    for (let oy = -1; oy <= 1; oy++) {
      for (let ox = -1; ox <= 1; ox++) {
        const bucket = grid.get((gy + oy) * 1024 + (gx + ox));
        if (!bucket) continue;
        for (const j of bucket) {
          if (j <= i) continue;
          const b = cnNodes[j];
          const dx = b.x - a.x;
          const dy = b.y - a.y;
          const d = Math.hypot(dx, dy) || 0.001;
          const min = a.r + b.r + COLLIDE_PAD * 1.9 * cnScale;
          if (d >= min) continue;
          const push = ((min - d) / d) * 0.5 * collideK * cnScale;
          const fx = dx * push;
          const fy = dy * push;
          if (!a.fixed) {
            a.vx -= fx;
            a.vy -= fy;
          }
          if (!b.fixed) {
            b.vx += fx;
            b.vy += fy;
          }
        }
      }
    }
  }

  // Every neighbour keeps returning to its own seat on the wide ellipse — the
  // flower becomes an even fan spread sideways, and the core stays in the
  // middle of it. The core and the pre-pump wallets keep their own seats too,
  // so the red bundle stays tight instead of flying apart.
  for (const nd of cnNodes) {
    if (nd.fixed || !nd.home) continue;
    const seat =
      nd.role === "core" || nd.role === "prepump"
        ? SEAT_PULL * cnScale
        : nd.role === "middle" && nd.relation === "group"
          ? RING_PULL * cnScale
          : nd.role === "middle" && nd.relation === "asset"
            ? COHORT_PULL * cnScale
            : 0;
    if (!seat) continue;
    nd.vx += (nd.home.x - nd.x) * seat * cnAlpha;
    nd.vy += (nd.home.y - nd.y) * seat * cnAlpha;
  }

  // Integrate.
  for (const nd of cnNodes) {
    if (nd.fixed) {
      // Притиснутий вузол стоїть там, куди його поставили (d3.drag у схемі).
      nd.vx = 0;
      nd.vy = 0;
      continue;
    }
    nd.vx *= 0.55;
    nd.vy *= 0.55;
    nd.x += nd.vx;
    nd.y += nd.vy;
    // The borders of the simulation space scale with the space itself.
    const pad = 40 * cnScale;
    nd.x = Math.max(pad, Math.min(SIM_W * cnScale - pad, nd.x));
    nd.y = Math.max(pad, Math.min(SIM_H * cnScale - pad, nd.y));
  }
  cnAlpha *= 0.992;
}

/** Which nodes are drawn: everything, or only the core and the pre-pump ones. */
export function capitalNetworkFilterState(): boolean {
  return cnCoreOnly;
}

export function setCapitalNetworkFilter(coreOnly: boolean): void {
  cnCoreOnly = coreOnly;
  const btn = $("#cn-filter");
  if (btn) {
    btn.classList.toggle("bg-accent", coreOnly);
    btn.classList.toggle("text-white", coreOnly);
  }
  start2D();
}

/** Nodes that are drawn right now. */
function visibleNodes(): SimNode[] {
  if (!cnCoreOnly) return cnNodes;
  return cnNodes.filter((n) => n.role === "core" || n.role === "prepump" || n.role === "hub");
}

/* ------------------------------------------------------------------ legend */

function renderLegend(d: CnData): void {
  const box = $("#cn-legend");
  if (!box) return;
  const items = d.legend || [];
  box.innerHTML = items
    .map((it) => {
      const label = legendLabel(it.key, it.label);
      return `<div class="flex items-center gap-2"><span class="inline-block w-2.5 h-2.5 rounded-full" style="background:${escAttr(it.color)}"></span><span>${escAttr(label)}</span></div>`;
    })
    .join("");
}

function legendLabel(key: string, fallback?: string): string {
  const map: Record<string, string> = {
    hub: "cn_role_hub",
    core: "cn_role_core",
    prepump: "cn_role_prepump",
    middle: "cn_role_middle",
  };
  const k = map[key];
  return k ? t(k) : fallback || key;
}

function renderStats(d: CnData): void {
  const box = $("#cn-stats");
  const status = $("#cn-status");
  const s = d.stats || {};
  const m = d.metadata || {};
  const group = String(m.groupLabel || m.group || "—");
  if (status) status.textContent = group;
  if (!box) return;
  const rows: string[] = [];
  rows.push(`<div class="font-semibold text-slate-200">${escAttr(group)}</div>`);
  rows.push(`<div class="text-slate-400 break-all">${escAttr(String(m.address || ""))}</div>`);
  rows.push(
    `<div class="text-slate-400">${escAttr(t("cn_wallets"))}: ${escAttr(String(s.members ?? "—"))}` +
      ` · ${escAttr(t("cn_role_core"))}: ${escAttr(String(s.core ?? 0))}` +
      ` · ${escAttr(t("cn_role_prepump"))}: ${escAttr(String(s.prepump ?? 0))}</div>`,
  );
  if (typeof s.totalUsd === "number" && s.totalUsd > 0) {
    rows.push(`<div class="text-slate-400">${escAttr(fmtUsd(s.totalUsd))}</div>`);
  }
  box.innerHTML = rows.join("");
}

/* --------------------------------------------------------------- 2D canvas */

/**
 * The info panel of the reference picture — top left, the address of the
 * wallet under the cursor (or the hub), its tokens, balance and prices.
 */
function renderInfo(nd?: SimNode | null): void {
  const box = $("#cn-info");
  if (!box) return;
  const node = nd || cnNodes.find((n) => n.type === "hub") || cnNodes[0];
  if (!node) {
    box.innerHTML = "";
    return;
  }
  // Панель метаданих — точно як у схемі: мітка зліва, значення справа.
  const tokens = (node.tokens || []).join(", ");
  const rows: [string, string][] = [
    [t("cn_address"), shortAddr(node.address)],
    [t("cn_tokens"), tokens || "—"],
    [t("cn_balance"), fmtUsd(node.balanceUsd || 0)],
    [t("cn_avg_buy"), "0"],
    [t("cn_avg_sell"), "—"],
  ];
  box.innerHTML = rows
    .map(([label, value]) =>
      `<div class="flex justify-between"><span class="text-[#6C8EBF]">${escAttr(label)}:</span><span class="ml-4 text-white">${escAttr(value)}</span></div>`)
    .join("");
}

function start2D(): void {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  canvas.classList.remove("hidden");

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.floor(rect.width * dpr));
    canvas.height = Math.max(1, Math.floor(rect.height * dpr));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  };
  resize();
  window.addEventListener("resize", resize);
  fitView();
  cancelAnimationFrame(cnRaf);
  // Timing of the last frame — how long the picture took to draw and how long
  // the browser waited for it. Handy when a group grows to a thousand wallets.
  let lastFrame = performance.now();
  const loop = () => {
    const t = performance.now();
    const wait = t - lastFrame;
    lastFrame = t;
    draw2D(ctx, canvas.clientWidth, canvas.clientHeight);
    const drawn = performance.now() - t;
    const w = window as unknown as { __cnPerf?: { wait: number; draw: number } };
    w.__cnPerf = { wait: Math.round(wait), draw: Math.round(drawn) };
    cnRaf = requestAnimationFrame(loop);
  };
  cnRaf = requestAnimationFrame(loop);
  installBenchmarks();
}

/** A direct benchmark of the drawing and of the simulation — the frame rate
 *  cannot be measured in a throttled (background) tab. */
function installBenchmarks(): void {
  const w = window as unknown as {
    __cnBench?: (frames?: number) => { frames: number; ms: number };
    __cnSim?: (ticks?: number) => { ticks: number; ms: number };
  };
  w.__cnBench = (frames = 5) => {
    const canvas = $<HTMLCanvasElement>("#cn-canvas");
    if (!canvas) return { frames: 0, ms: 0 };
    const ctx = canvas.getContext("2d");
    if (!ctx) return { frames: 0, ms: 0 };
    // The cheapest of the runs is the honest one: the others wait for the
    // compositor to catch up with the backlog of commands.
    let best = Infinity;
    for (let i = 0; i < frames; i++) {
      const t0 = performance.now();
      draw2D(ctx, canvas.clientWidth, canvas.clientHeight);
      const ms = performance.now() - t0;
      if (ms < best) best = ms;
    }
    return { frames, ms: best };
  };
  w.__cnSim = (ticks = 10) => {
    const t0 = performance.now();
    for (let i = 0; i < ticks; i++) tick();
    const t1 = performance.now();
    return { ticks, ms: (t1 - t0) / ticks };
  };
}

function stop2D(): void {
  cancelAnimationFrame(cnRaf);
}

/** Fit the picture into the canvas. */
export function fitCapitalNetworkView(): void {
  fitView();
}

function fitView(): void {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return;
  const W = canvas.clientWidth || 1;
  const H = canvas.clientHeight || 1;
  const ns = visibleNodes();
  if (!ns.length) return;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of ns) {
    minX = Math.min(minX, n.x);
    maxX = Math.max(maxX, n.x);
    minY = Math.min(minY, n.y);
    maxY = Math.max(maxY, n.y);
  }
  // The panels sit on both sides of the screen — keep the outer wallets clear
  // of them, but do not waste the middle of the picture.
  const pad = 70;
  const scale = Math.min(
    (W - pad * 2) / Math.max(1, maxX - minX),
    (H - pad * 2) / Math.max(1, maxY - minY),
  );
  cnView.scale = scale;
  cnView.ox = W / 2 - ((minX + maxX) / 2) * scale;
  cnView.oy = H / 2 - ((minY + maxY) / 2) * scale;
}

function toScreen(x: number, y: number, W: number, H: number): [number, number] {
  return [x * cnView.scale + cnView.ox, y * cnView.scale + cnView.oy];
}

function draw2D(ctx: CanvasRenderingContext2D, W: number, H: number): void {
  const bg = cnData?.styles?.background?.color || "#1A1D21";
  ctx.globalCompositeOperation = "source-over";
  ctx.globalAlpha = 1;
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  // The dot grid of the reference — in screen space, so it never scales.
  const grid = cnData?.styles?.background?.grid_pattern;
  const spacing = grid?.spacing || 20;
  ctx.fillStyle = grid?.color || "#2A2E35";
  for (let x = spacing / 2; x < W; x += spacing) {
    for (let y = spacing / 2; y < H; y += spacing) ctx.fillRect(x, y, 1, 1);
  }

  // The indices, not the wallets — a group with a thousand wallets must not
  // walk the whole list for every one of them.
  const vis: number[] = [];
  for (let i = 0; i < cnNodes.length; i++) {
    const nd = cnNodes[i];
    if (cnCoreOnly && nd.role !== "core" && nd.role !== "prepump" && nd.role !== "hub") continue;
    vis.push(i);
  }
  const visibleIdx = new Set(vis);

  // Links — with `screen` blending they glow where they cross, exactly like
  // the reference picture. The lines are drawn in batches, one path per
  // style: six thousand wallets would otherwise mean six thousand strokes.
  // The `screen` blend of the reference is beautiful but expensive: with
  // thousands of lines it costs more than the whole rest of the picture.
  const huge = cnNodes.length > 2000;
  const blend = (
    huge ? "source-over" : cnData?.styles?.link?.mix_blend_mode || "screen"
  ) as GlobalCompositeOperation;
  ctx.globalCompositeOperation = blend;
  const batches = new Map<string, { color: string; width: number; alpha: number; pts: number[] }>();
  for (const l of cnLinks) {
    if (!visibleIdx.has(l.s) || !visibleIdx.has(l.t)) continue;
    const key = `${l.type}|${l.color}|${l.width}|${l.opacity}`;
    let batch = batches.get(key);
    if (!batch) {
      batch = { color: l.color, width: l.width, alpha: l.opacity, pts: [] };
      batches.set(key, batch);
    }
    const a = cnNodes[l.s];
    const b = cnNodes[l.t];
    const [x1, y1] = toScreen(a.x, a.y, W, H);
    const [x2, y2] = toScreen(b.x, b.y, W, H);
    batch.pts.push(x1, y1, x2, y2);
  }
  for (const batch of batches.values()) {
    ctx.globalAlpha = batch.alpha;
    ctx.strokeStyle = batch.color;
    ctx.lineWidth = batch.width;
    ctx.beginPath();
    const p = batch.pts;
    for (let i = 0; i < p.length; i += 4) {
      ctx.moveTo(p[i], p[i + 1]);
      ctx.lineTo(p[i + 2], p[i + 3]);
    }
    ctx.stroke();
  }
  ctx.globalCompositeOperation = "source-over";
  ctx.globalAlpha = 1;

  // Nodes — hub over special over satellite. The glow is the expensive part
  // of the drawing, so only the wallets that carry a role get it; the ring of
  // satellites is drawn as one path.
  const order = vis.slice().sort((a, b) => zIndex(cnNodes[a]) - zIndex(cnNodes[b]));
  let plainColor = "";
  let plainStroke = "";
  ctx.beginPath();
  for (const i of order) {
    const nd = cnNodes[i];
    if (isGlow(nd)) continue;
    const [x, y] = toScreen(nd.x, nd.y, W, H);
    const r = Math.max(1.2, nd.r * cnView.scale * 0.72);
    ctx.moveTo(x + r, y);
    ctx.arc(x, y, r, 0, Math.PI * 2);
    if (!plainColor) {
      plainColor = nd.color;
      plainStroke = nd.stroke;
    }
  }
  if (plainColor) {
    // У схемі сусіди трохи прозоріші (opacity 0.9).
    ctx.globalAlpha = 0.9;
    ctx.fillStyle = plainColor;
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.strokeStyle = plainStroke;
    ctx.lineWidth = 1.4;
    ctx.stroke();
  }

  for (const i of order) {
    const nd = cnNodes[i];
    if (!isGlow(nd)) continue;
    const [x, y] = toScreen(nd.x, nd.y, W, H);
    const r = nd.r * cnView.scale * 0.72;
    ctx.beginPath();
    ctx.arc(x, y, Math.max(1.2, r), 0, Math.PI * 2);
    ctx.shadowBlur = nd.type === "hub" ? 22 : huge ? 0 : 14;
    ctx.shadowColor = nd.color;
    ctx.fillStyle = nd.color;
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.shadowBlur = 0;
    ctx.strokeStyle = nd.stroke;
    ctx.lineWidth = 1.4;
    ctx.stroke();
  }

  // The hovered node gets a ring.
  if (cnHover >= 0 && visibleIdx.has(cnHover)) {
    const nd = cnNodes[cnHover];
    const [x, y] = toScreen(nd.x, nd.y, W, H);
    ctx.beginPath();
    ctx.arc(x, y, Math.max(6, nd.r * cnView.scale * 0.9), 0, Math.PI * 2);
    ctx.strokeStyle = "#FFFFFF";
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }
}

/** The wallets that carry a role — they are drawn with a glow. */
function isGlow(n: SimNode): boolean {
  return n.type === "hub" || n.role === "core" || n.role === "prepump" || n.role === "cohort-hub";
}

function zIndex(n: SimNode): number {
  if (n.type === "hub") return 3;
  if (n.type === "special-red" || n.type === "special-green") return 2;
  return 1;
}

/* ------------------------------------------------------------- interaction */

export function setupCapitalNetworkInteractions(): void {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return;

  canvas.addEventListener("wheel", (e: WheelEvent) => {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    cnView.scale = Math.max(0.15, Math.min(12, cnView.scale * factor));
  }, { passive: false });

  canvas.addEventListener("pointerdown", (e: PointerEvent) => {
    // Спершу — перетягування вузла (d3.drag у схемі), інакше — переміщення фону.
    const idx = nodeAt(e.clientX, e.clientY);
    if (idx >= 0) {
      cnDrag = { idx, x: e.clientX, y: e.clientY };
      cnNodes[idx].fixed = true;
      canvas.setPointerCapture(e.pointerId);
      return;
    }
    cnPan = { x: e.clientX, y: e.clientY, ox: cnView.ox, oy: cnView.oy };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e: PointerEvent) => {
    if (cnDrag) {
      const canvasEl = $<HTMLCanvasElement>("#cn-canvas");
      if (!canvasEl) return;
      const rect = canvasEl.getBoundingClientRect();
      const nd = cnNodes[cnDrag.idx];
      nd.x = (e.clientX - rect.left - cnView.ox) / cnView.scale;
      nd.y = (e.clientY - rect.top - cnView.oy) / cnView.scale;
      nd.vx = 0;
      nd.vy = 0;
      cnAlpha = Math.max(cnAlpha, 0.25);
      return;
    }
    if (cnPan) {
      cnView.ox = cnPan.ox + (e.clientX - cnPan.x);
      cnView.oy = cnPan.oy + (e.clientY - cnPan.y);
      return;
    }
    const idx = nodeAt(e.clientX, e.clientY);
    if (idx !== cnHover) {
      cnHover = idx;
      renderInfo(idx >= 0 ? cnNodes[idx] : null);
      if (idx >= 0) showTooltip(idx, e.clientX, e.clientY);
      else hideTooltip();
    } else if (idx >= 0) {
      moveTooltip(e.clientX, e.clientY);
    }
  });
  const endPan = () => {
    cnPan = null;
    if (cnDrag) {
      // Відпустили вузол — він знову вільний (як d3.drag у схемі).
      const nd = cnNodes[cnDrag.idx];
      if (nd) nd.fixed = false;
      cnDrag = null;
    }
  };
  canvas.addEventListener("pointerup", endPan);
  canvas.addEventListener("pointercancel", endPan);
  canvas.addEventListener("pointerleave", () => {
    cnHover = -1;
    renderInfo(null);
    hideTooltip();
  });
  canvas.addEventListener("click", (e: MouseEvent) => {
    const idx = nodeAt(e.clientX, e.clientY);
    if (idx >= 0) {
      renderInfo(cnNodes[idx]);
      showDetail(cnNodes[idx]);
    }
  });
  canvas.addEventListener("dblclick", (e: MouseEvent) => {
    const idx = nodeAt(e.clientX, e.clientY);
    if (idx >= 0 && cnNodes[idx].address) openGraphForWallet(cnNodes[idx].address);
  });
}

/** The node under the pointer, if any. */
function nodeAt(px: number, py: number): number {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return -1;
  const rect = canvas.getBoundingClientRect();
  const x = px - rect.left;
  const y = py - rect.top;
  const W = canvas.clientWidth;
  const H = canvas.clientHeight;
  let best = -1;
  let bestD = Infinity;
  for (let i = 0; i < cnNodes.length; i++) {
    const nd = cnNodes[i];
    if (cnCoreOnly && nd.role !== "core" && nd.role !== "prepump" && nd.role !== "hub") continue;
    const [sx, sy] = toScreen(nd.x, nd.y, W, H);
    const d = Math.hypot(sx - x, sy - y);
    const hit = Math.max(8, nd.r * cnView.scale * 0.75);
    if (d <= hit && d < bestD) {
      bestD = d;
      best = i;
    }
  }
  return best;
}

/** The tooltip of the reference: address, tokens, balance, average prices. */
function showTooltip(idx: number, px: number, py: number): void {
  const tip = $("#cn-tooltip");
  if (!tip) return;
  const nd = cnNodes[idx];
  const tokens = (nd.tokens || []).join(", ") || "—";
  const roleKey = `cn_role_${nd.role || "middle"}`;
  const role = t(roleKey);
  const share = typeof nd.share === "number" && nd.share > 0
    ? `<div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_share"))}:</span>
      <span>${escAttr(nd.share.toFixed(2))}%</span></div>`
    : "";
  tip.innerHTML = `
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_address"))}:</span>
      <span class="text-accent font-mono">${escAttr(shortAddr(nd.address))}</span></div>
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_role"))}:</span>
      <span>${escAttr(role)}</span></div>
    ${share}
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_tokens"))}:</span>
      <span>${escAttr(tokens)}</span></div>
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_balance"))}:</span>
      <span>${escAttr(fmtUsd(nd.balanceUsd || 0))}</span></div>
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_avg_buy"))}:</span>
      <span>0</span></div>
    <div class="flex justify-between gap-6"><span class="text-slate-400">${escAttr(t("cn_avg_sell"))}:</span>
      <span>—</span></div>`;
  tip.classList.remove("hidden");
  moveTooltip(px, py);
}

function moveTooltip(px: number, py: number): void {
  const tip = $("#cn-tooltip");
  if (!tip) return;
  const w = tip.offsetWidth || 220;
  const h = tip.offsetHeight || 120;
  tip.style.left = `${Math.min(window.innerWidth - w - 12, px + 16)}px`;
  tip.style.top = `${Math.max(12, py - h - 12)}px`;
}

function hideTooltip(): void {
  const tip = $("#cn-tooltip");
  if (tip) tip.classList.add("hidden");
}

/** The detail panel of a clicked node. */
function showDetail(nd: SimNode): void {
  const panel = $("#cn-detail");
  if (!panel) return;
  const rows: string[] = [];
  rows.push(`<div class="font-semibold text-slate-100">${escAttr(nd.label || shortAddr(nd.address))}</div>`);
  rows.push(`<div class="text-xs text-accent font-mono break-all">${escAttr(nd.address)}</div>`);
  rows.push(`<div class="text-xs text-slate-400 mt-2">${escAttr(t("cn_role"))}: ${escAttr(roleLabel(nd.role))}</div>`);
  if (nd.degree !== undefined) {
    rows.push(`<div class="text-xs text-slate-400">${escAttr(t("cn_degree"))}: ${escAttr(String(nd.degree))}</div>`);
  }
  if (nd.internalDegree !== undefined) {
    rows.push(`<div class="text-xs text-slate-400">${escAttr(t("cn_links_inside"))}: ${escAttr(String(nd.internalDegree))}</div>`);
  }
  if (nd.share) {
    rows.push(`<div class="text-xs text-slate-400">${escAttr(t("cn_share"))}: ${escAttr(nd.share.toFixed(2))}%</div>`);
  }
  rows.push(`<div class="text-xs text-slate-400">${escAttr(t("cn_balance"))}: ${escAttr(fmtUsd(nd.balanceUsd || 0))}</div>`);
  if (nd.tokens && nd.tokens.length) {
    rows.push(`<div class="text-xs text-slate-400">${escAttr(t("cn_tokens"))}: ${escAttr(nd.tokens.join(", "))}</div>`);
  }
  rows.push(
    `<button class="mt-3 text-xs text-accent hover:underline" data-cn-open-graph="${escAttr(nd.address)}">${escAttr(t("cn_open_graph"))}</button>`,
  );
  panel.innerHTML = rows.join("");
  panel.classList.remove("translate-x-full");
  panel.querySelector("[data-cn-open-graph]")?.addEventListener("click", () => {
    if (nd.address) openGraphForWallet(nd.address);
  });
}

function roleLabel(role: string): string {
  const map: Record<string, string> = {
    hub: "cn_role_hub",
    core: "cn_role_core",
    prepump: "cn_role_prepump",
    middle: "cn_role_middle",
  };
  const k = map[role];
  return k ? t(k) : role;
}

