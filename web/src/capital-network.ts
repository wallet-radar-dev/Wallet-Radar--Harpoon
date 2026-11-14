/* ---------- «Структура групи капіталу та пов'язаних гаманців» ----------
   Радіальна кластерна мережа за інструкцією: жовта група капіталу в центрі
   (хаб у самому центрі, дотично пов'язані гаманці — радіально), сині вторинні
   кластери навколо, червоні виділені вузли всередині групи капіталу, зелені
   вузли-з'єдначі.

   Розкладку рахує бекенд (/api/capital-network) з реальних груп і крафт-графа:
   позиція кластера залежить від кількості хопів до групи капіталу, позиція
   гаманця — від його участі в групі. Тут лише малюнок: два режими (2D canvas
   і 3D Three.js) поверх однієї й тієї ж розкладки. */

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

import { t } from "./i18n";
import { $, shortAddr, fmtUsd } from "./util";
import { escAttr } from "./ui";
import { getJSON } from "./api";
import { openGraphForWallet } from "./graph";

/* ---------- типи відповіді ---------- */

interface CnPos { x: number; y: number; z: number }

interface CnNode {
  address: string; label: string | null; role: string;
  degree: number; internalDegree: number; participation: number; txWithHub: number;
  hops: number | null; tokens: string[]; balanceUsd: number; pos: CnPos;
}

interface CnCluster {
  id: string; label: string; kind: string; color: string;
  size: number; memberCount: number; shownMembers: number; totalUsd: number;
  assets: string[]; hops: number; position: string;
  linked?: boolean;
  hub: { address: string; label: string | null; degree: number; balanceUsd: number; pos: CnPos };
  pos: CnPos; members: CnNode[];
}

interface CnEdge { from: string; to: string; weight: number; kind: string; volumeUsd?: number }
interface CnCross { from: string; to: string; via: string; through?: string; weight: number; hops: number; kind: string }
interface CnHighlight { address: string; label: string | null; reason: string; internalDegree: number; participation: number; degree: number; balanceUsd: number }
interface CnConn { address: string; label: string | null; groups: string[]; holderCount: number; assetCount: number; maxBackerUsd: number; position: string; pos: CnPos }

interface CnData {
  generated: number; blockchain: string; visualization: string;
  main: CnCluster; groups: CnCluster[];
  edges: CnEdge[]; crossLinks: CnCross[];
  special: { highlighted: CnHighlight[]; connectionNodes: CnConn[] };
  specialEdges: { from: string; to: string; weight: number; kind: string }[];
  legend: Record<string, string>;
  stats: Record<string, number>;
  analysis?: Record<string, unknown>;
  dataReliability?: Record<string, string>;
}

/* ---------- модель для малювання ---------- */

type DrawKind = "hub" | "core" | "peripheral" | "member" | "clusterHub" | "special" | "highlight";

interface DrawNode {
  addr: string; label: string | null;
  x: number; y: number; z: number;
  r: number; color: string; kind: DrawKind;
  degree: number; balanceUsd: number; tokens: string[];
  hops: number | null; participation: number; internalDegree: number;
  group: string | null; groupLabel: string | null;
  dim?: boolean;
}

interface DrawEdge { a: number; b: number; color: string; alpha: number; w: number }
interface DrawCluster { id: string; label: string; x: number; y: number; color: string; size: number; kind: string; hops: number; totalUsd: number; showLabel: boolean; linked: boolean }
interface DrawCross { from: string; to: string; hops: number }

const COL = {
  yellow: "#facc15",
  yellowHub: "#fde047",
  blue: "#38bdf8",
  blueHub: "#7dd3fc",
  red: "#ef4444",
  green: "#22c55e",
};

/* ---------- стан модуля ---------- */

let cnData: CnData | null = null;
let cnMode: "2d" | "3d" = "2d";
let cnLinkedOnly = true;
let cnOpen = false;
let cnLoading = false;
let cnRaf = 0;
let cnView = { scale: 1, ox: 0, oy: 0 };
let cnNodes: DrawNode[] = [];
let cnEdges: DrawEdge[] = [];
let cnClusters: DrawCluster[] = [];
let cnCross: DrawCross[] = [];
let cnByAddr = new Map<string, number>();
let cnHoverIdx = -1;
let cnPointer = { x: 0, y: 0, inside: false };
let cnPan: { x: number; y: number; ox: number; oy: number } | null = null;

/* 3D */
let cnThree: {
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  points: THREE.Points;
  edges: THREE.LineSegments;
  cross: THREE.LineSegments;
  raf: number;
} | null = null;

/* ---------- відкриття / закриття ---------- */

export function openCapitalNetwork(): void {
  const modal = $("#capital-modal");
  if (!modal) return;
  modal.classList.remove("hidden");
  modal.classList.add("flex");
  cnOpen = true;
  cnView = { scale: 1, ox: 0, oy: 0 };
  void loadCapitalNetwork();
}

export function closeCapitalNetwork(): void {
  const modal = $("#capital-modal");
  if (modal) {
    modal.classList.add("hidden");
    modal.classList.remove("flex");
  }
  cnOpen = false;
  cnHoverIdx = -1;
  cnPointer.inside = false;
  dispose3D();
  cancelAnimationFrame(cnRaf);
  cnRaf = 0;
  $("#cn-detail")?.classList.add("translate-x-full");
  $("#cn-canvas")?.classList.remove("hidden");
  const mount = $("#cn-3d");
  if (mount) mount.classList.add("hidden");
}

/** Перезавантаження структури (кнопка ⟳). */
export async function reloadCapitalNetwork(): Promise<void> {
  await loadCapitalNetwork();
}

/* ---------- дані ---------- */

async function loadCapitalNetwork(): Promise<void> {
  if (cnLoading) return;
  cnLoading = true;
  const status = $("#cn-status");
  if (status) status.textContent = t("cn_loading");
  try {
    const d = await getJSON<CnData>("/api/capital-network");
    cnData = d;
    buildModel(d);
    fitView();
    renderLegend(d);
    renderStats(d);
    updateFilterButton(d);
    if (status) status.textContent = "";
    if (cnMode === "3d") mount3D();
    else start2D();
  } catch (e) {
    if (status) status.textContent = t("cn_error");
    console.error("[capital-network]", e);
  } finally {
    cnLoading = false;
  }
}

/** Розкладає відповідь у модель для малювання: позиції приходять від бекенду. */
function buildModel(d: CnData): void {
  cnNodes = [];
  cnEdges = [];
  cnClusters = [];
  cnCross = [];
  cnByAddr = new Map();

  const push = (n: DrawNode): number => {
    cnNodes.push(n);
    cnByAddr.set(n.addr, cnNodes.length - 1);
    return cnNodes.length - 1;
  };

  const highlighted = new Set((d.special?.highlighted || []).map((h) => h.address));

  // Головна група капіталу — жовта, у центрі.
  const main = d.main;
  for (const m of main.members) {
    const isHub = m.role === "hub";
    const isHl = highlighted.has(m.address);
    push({
      addr: m.address, label: m.label,
      x: m.pos.x, y: m.pos.y, z: m.pos.z,
      r: isHub ? 9 : Math.min(6, 2.6 + Math.sqrt(m.degree) * 0.55),
      color: isHl ? COL.red : isHub ? COL.yellowHub : COL.yellow,
      kind: isHl ? "highlight" : isHub ? "hub" : m.role === "core" ? "core" : "peripheral",
      degree: m.degree, balanceUsd: m.balanceUsd, tokens: m.tokens || [],
      hops: m.hops, participation: m.participation, internalDegree: m.internalDegree,
      group: main.id, groupLabel: main.label,
    });
  }

  // Вторинні кластери — сині (капітальні лишаються жовтими).
  // Мітку показуємо лише у найважливіших: інакше 545 написів — каша.
  const labelled = new Set(
    [...d.groups || []]
      .sort((a, b) => b.totalUsd - a.totalUsd)
      .slice(0, 12)
      .map((g) => g.id),
  );
  for (const g of d.groups || []) {
    const linked = g.linked !== false;
    // За замовчуванням показуємо лише кластери, зв'язані з групою капіталу:
    // решта — це окремі компоненти без спільних гаманців, і вони лише засмічують
    // малюнок. Їх лишається увімкнути кнопкою-фільтром.
    if (cnLinkedOnly && !linked) continue;
    cnClusters.push({
      id: g.id, label: g.label, x: g.pos.x, y: g.pos.y,
      color: g.color === "yellow" ? COL.yellow : COL.blue,
      size: g.size, kind: g.kind, hops: g.hops, totalUsd: g.totalUsd,
      showLabel: labelled.has(g.id) && linked,
      linked,
    });
    for (const m of g.members) {
      if (cnByAddr.has(m.address)) continue;
      const isHub = m.role === "hub";
      push({
        addr: m.address, label: m.label,
        x: m.pos.x, y: m.pos.y, z: m.pos.z,
        r: (isHub ? 5.5 : Math.min(4, 1.8 + Math.sqrt(m.degree) * 0.4)) * (linked ? 1 : 0.7),
        color: isHub ? COL.blueHub : COL.blue,
        kind: isHub ? "clusterHub" : "member",
        degree: m.degree, balanceUsd: m.balanceUsd, tokens: m.tokens || [],
        hops: m.hops, participation: m.participation, internalDegree: m.internalDegree,
        group: g.id, groupLabel: g.label,
        dim: !linked,
      });
    }
  }

  // Зелені вузли-з'єдначі: провідники, що зв'язують кілька груп.
  // Три «опорні» (top_center, lower_center, lower_right) — як у референсі,
  // решта — приглушені.
  for (const c of d.special?.connectionNodes || []) {
    const primary = c.position !== "ring";
    push({
      addr: c.address, label: c.label,
      x: c.pos.x, y: c.pos.y, z: c.pos.z,
      r: primary ? 7 : 4.5, color: COL.green, kind: "special",
      degree: 0, balanceUsd: 0, tokens: [],
      hops: null, participation: c.groups.length, internalDegree: 0,
      group: null, groupLabel: null,
      dim: !primary,
    });
  }

  // Ребра всередині групи капіталу: сині — прямі зв'язки з хабом,
  // червоні — зв'язки між гаманцями.
  for (const e of d.edges || []) {
    const a = cnByAddr.get(e.from);
    const b = cnByAddr.get(e.to);
    if (a === undefined || b === undefined) continue;
    cnEdges.push({
      a, b,
      color: e.kind === "inter_wallet" ? COL.red : COL.blue,
      alpha: e.kind === "inter_wallet" ? 0.22 : 0.4,
      w: Math.min(3, 0.6 + Math.log10(1 + e.weight) * 0.9),
    });
  }

  // Крос-зв'язки між кластерами.
  for (const c of d.crossLinks || []) {
    cnCross.push({ from: c.from, to: c.to, hops: c.hops });
  }
  // Зелені вузли-з'єдначі пов'язані з хабом групи капіталу.
  for (const e of d.specialEdges || []) {
    const a = cnByAddr.get(e.from);
    const b = cnByAddr.get(e.to);
    if (a === undefined || b === undefined) continue;
    cnEdges.push({ a, b, color: COL.green, alpha: 0.5, w: 1.4 });
  }
}

/* ---------- легенда та статистика ---------- */

function renderLegend(d: CnData): void {
  const el = $("#cn-legend");
  if (!el) return;
  const items: Array<[string, string]> = [
    [COL.yellow, d.legend?.yellow || t("cn_legend_yellow")],
    [COL.blue, d.legend?.blue || t("cn_legend_blue")],
    [COL.red, d.legend?.red || t("cn_legend_red")],
    [COL.green, d.legend?.green || t("cn_legend_green")],
  ];
  el.innerHTML = items.map(([c, label]) =>
    `<div class="flex items-center gap-2"><span style="width:10px;height:10px;border-radius:50%;background:${c};display:inline-block"></span><span class="text-slate-300">${label}</span></div>`
  ).join("");
}

function renderStats(d: CnData): void {
  const el = $("#cn-stats");
  if (!el) return;
  const s = d.stats || {};
  const main = d.main;
  const a = d.analysis as { linkedGroups?: number; maxHops?: number } | undefined;
  const r = d.dataReliability as { walletOwnership?: string; transactionDirection?: string; relationships?: string } | undefined;
  el.innerHTML = `
    <div class="text-slate-300 font-medium">${main.id} · ${main.size} ${t("cn_wallets")}</div>
    <div class="text-slate-400">${fmtUsd(main.totalUsd)} · ${t("cn_hub")}: ${shortAddr(main.hub.address)}</div>
    <div class="text-slate-500">${t("cn_clusters")}: ${s.groups ?? 0} · ${t("cn_edges")}: ${s.edges ?? 0}</div>
    ${a?.linkedGroups !== undefined ? `<div class="text-slate-500">${t("cn_linked")}: ${a.linkedGroups} · ${t("cn_max_hops")}: ${a.maxHops ?? "—"}</div>` : ""}
    ${r?.walletOwnership ? `<div class="text-slate-600 mt-1 max-w-[240px]">${r.walletOwnership} · ${r.transactionDirection ?? ""}</div>` : ""}`;
}

/** Поточний стан фільтра — для кнопки-перемикача. */
export function capitalNetworkFilterState(): boolean {
  return cnLinkedOnly;
}

/** Фільтр: лише кластери, зв'язані з групою капіталу, або всі досліджувані. */
export function setCapitalNetworkFilter(linkedOnly: boolean): void {
  cnLinkedOnly = linkedOnly;
  if (!cnData) return;
  buildModel(cnData);
  fitView();
  renderStats(cnData);
  updateFilterButton(cnData);
  if (cnMode === "3d") {
    dispose3D();
    void mount3D();
  }
}

function updateFilterButton(d: CnData): void {
  const btn = $("#cn-filter");
  if (!btn) return;
  const total = d.groups?.length ?? 0;
  const linked = (d.groups || []).filter((g) => g.linked !== false).length;
  btn.textContent = cnLinkedOnly ? `🔗 ${linked}` : `🌐 ${total}`;
  btn.classList.toggle("border-accent", cnLinkedOnly);
}

/** Показати структуру повністю (кнопка ⌂) — без повторного запиту до сервера. */
export function fitCapitalNetworkView(): void {
  fitView();
}

/** Підганяє масштаб так, щоб у кадр вмістилася група капіталу з першими
 *  кільцями кластерів (далекі кластери лишаються «за межею» — їх видно з масштабом). */
function fitView(): void {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return;
  const rect = canvas.getBoundingClientRect();
  const W = rect.width || 1200;
  const H = rect.height || 800;
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  const consider = (x: number, y: number): void => {
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  };
  for (const n of cnNodes) {
    if (n.kind === "hub" || n.kind === "core" || n.kind === "highlight") consider(n.x, n.y);
  }
  for (const c of cnClusters) {
    if (c.hops <= 1) consider(c.x, c.y);
  }
  if (!isFinite(minX) || !isFinite(maxX)) return;
  const w = Math.max(0.5, maxX - minX);
  const h = Math.max(0.5, maxY - minY);
  const scale = Math.min(W / (w * 1.15), H / (h * 1.15));
  cnView.scale = Math.max(0.05, Math.min(600, scale));
  cnView.ox = -((minX + maxX) / 2) * cnView.scale;
  cnView.oy = -((minY + maxY) / 2) * cnView.scale;
  // Службовий маркер — видно, який масштаб підібрав розкладник.
  canvas.dataset.cnScale = `${cnView.scale.toFixed(2)}`;
  canvas.dataset.cnBox = `${minX.toFixed(2)}..${maxX.toFixed(2)} / ${minY.toFixed(2)}..${maxY.toFixed(2)}`;
}

/* ---------- 2D-режим ---------- */

function start2D(): void {
  dispose3D();
  const canvas = $("#cn-canvas");
  if (canvas) canvas.classList.remove("hidden");
  const mount = $("#cn-3d");
  if (mount) mount.classList.add("hidden");
  if (!cnRaf) cnRaf = requestAnimationFrame(loop2D);
}

function loop2D(): void {
  cnRaf = requestAnimationFrame(loop2D);
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas || !cnOpen) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const rect = canvas.getBoundingClientRect();
  const W = Math.max(1, Math.round(rect.width));
  const H = Math.max(1, Math.round(rect.height));
  if (canvas.width !== W || canvas.height !== H) { canvas.width = W; canvas.height = H; }
  draw2D(ctx, W, H);
}

function toScreen(x: number, y: number, W: number, H: number): [number, number] {
  return [W / 2 + (x * cnView.scale + cnView.ox), H / 2 - (y * cnView.scale + cnView.oy)];
}

function draw2D(ctx: CanvasRenderingContext2D, W: number, H: number): void {
  ctx.clearRect(0, 0, W, H);
  ctx.fillStyle = "#0b0f1a";
  ctx.fillRect(0, 0, W, H);

  // Сітка — орієнтир масштабу.
  ctx.strokeStyle = "rgba(148,163,184,0.06)";
  ctx.lineWidth = 1;
  const step = 60 * cnView.scale;
  if (step > 12) {
    const ox = W / 2 + cnView.ox % step;
    const oy = H / 2 + cnView.oy % step;
    for (let x = ox; x < W; x += step) { ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, H); ctx.stroke(); }
    for (let y = oy; y < H; y += step) { ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(W, y); ctx.stroke(); }
  }

  // Крос-зв'язки між кластерами — пунктирні: більше хопів — довші пункси.
  ctx.setLineDash([6, 6]);
  for (const c of cnCross) {
    const a = cnByAddr.get(c.from);
    const b = cnByAddr.get(c.to);
    if (a === undefined || b === undefined) continue;
    const [ax, ay] = toScreen(cnNodes[a].x, cnNodes[a].y, W, H);
    const [bx, by] = toScreen(cnNodes[b].x, cnNodes[b].y, W, H);
    ctx.strokeStyle = c.hops > 1 ? "rgba(148,163,184,0.28)" : "rgba(56,189,248,0.45)";
    ctx.lineWidth = c.hops > 1 ? 1 : 1.6;
    ctx.beginPath(); ctx.moveTo(ax, ay); ctx.lineTo(bx, by); ctx.stroke();
  }
  ctx.setLineDash([]);

  // Внутрішні зв'язки.
  for (const e of cnEdges) {
    const n1 = cnNodes[e.a];
    const n2 = cnNodes[e.b];
    if (!n1 || !n2) continue;
    const [ax, ay] = toScreen(n1.x, n1.y, W, H);
    const [bx, by] = toScreen(n2.x, n2.y, W, H);
    ctx.strokeStyle = hexA(e.color, e.alpha);
    ctx.lineWidth = e.w;
    ctx.beginPath(); ctx.moveTo(ax, ay); ctx.lineTo(bx, by); ctx.stroke();
  }

  // Вузли.
  for (let i = 0; i < cnNodes.length; i++) {
    const n = cnNodes[i];
    const [x, y] = toScreen(n.x, n.y, W, H);
    if (x < -40 || x > W + 40 || y < -40 || y > H + 40) continue;
    const r = n.r * Math.min(1.5, Math.max(0.6, cnView.scale));
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fillStyle = hexA(n.color, n.dim ? 0.3 : n.kind === "peripheral" || n.kind === "member" ? 0.65 : 0.92);
    ctx.fill();
    if (i === cnHoverIdx) {
      ctx.strokeStyle = "#e2e8f0";
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
  }

  // Підписи кластерів — на «таблетці», щоб читались поверх точок.
  ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
  ctx.textAlign = "center";
  for (const c of cnClusters) {
    if (!c.showLabel) continue;
    const [x, y] = toScreen(c.x, c.y, W, H);
    if (x < -60 || x > W + 60 || y < -30 || y > H + 30) continue;
    const text = `${c.id} · ${c.size}`;
    const w = ctx.measureText(text).width + 10;
    ctx.fillStyle = "rgba(11,15,26,0.75)";
    ctx.beginPath();
    ctx.roundRect(x - w / 2, y - 16, w, 15, 4);
    ctx.fill();
    ctx.fillStyle = c.color;
    ctx.fillText(text, x, y - 5);
  }
  // Підпис центрального гаманця.
  const hubIdx = cnByAddr.get(cnData?.main.hub.address || "");
  if (hubIdx !== undefined) {
    const [x, y] = toScreen(cnNodes[hubIdx].x, cnNodes[hubIdx].y, W, H);
    ctx.fillStyle = "#fde047";
    ctx.textAlign = "center";
    ctx.fillText(shortAddr(cnNodes[hubIdx].addr), x, y - 14);
  }
}

function hexA(hex: string, a: number): string {
  const h = hex.replace("#", "");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${a})`;
}

/* ---------- 3D-режим ---------- */

async function mount3D(): Promise<void> {
  const canvas = $("#cn-canvas");
  if (canvas) canvas.classList.add("hidden");
  const mount = $("#cn-3d");
  if (!mount) return;
  mount.classList.remove("hidden");
  if (cnThree) { cnThree.controls.autoRotate = true; return; }

  const scene = new THREE.Scene();
  scene.background = new THREE.Color(0x0b0f1a);

  const camera = new THREE.PerspectiveCamera(48, 1, 0.1, 200);
  camera.position.set(0, -2.0, 2.9);

  // WebGL може бути недоступним (старі браузери, вимкнений апаратний
  // прискорювач, «-headless»-середовище). Тоді не лишаємо чорний екран, а
  // кажемо про це й повертаємо 2D.
  let renderer: THREE.WebGLRenderer;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true });
  } catch {
    mount.classList.add("hidden");
    canvas?.classList.remove("hidden");
    const note = $("#cn-3d-note");
    if (note) {
      note.textContent = t("cn_no_webgl");
      note.classList.remove("hidden");
      window.setTimeout(() => note.classList.add("hidden"), 6000);
    }
    setCapitalNetworkMode("2d");
    return;
  }
  renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
  mount.appendChild(renderer.domElement);
  renderer.domElement.style.width = "100%";
  renderer.domElement.style.height = "100%";
  renderer.domElement.style.display = "block";

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.autoRotate = true;
  controls.autoRotateSpeed = 0.35;
  controls.minDistance = 1.2;
  controls.maxDistance = 20;

  // Вузли — кольоровий Points.
  const sprite = roundSprite();
  const pos = new Float32Array(cnNodes.length * 3);
  const col = new Float32Array(cnNodes.length * 3);
  const c3 = new THREE.Color();
  cnNodes.forEach((n, i) => {
    pos[i * 3] = n.x; pos[i * 3 + 1] = n.y; pos[i * 3 + 2] = n.z;
    c3.set(n.color);
    if (n.dim) c3.multiplyScalar(0.35);
    col[i * 3] = c3.r; col[i * 3 + 1] = c3.g; col[i * 3 + 2] = c3.b;
  });
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
  geo.setAttribute("color", new THREE.BufferAttribute(col, 3));
  const points = new THREE.Points(geo, new THREE.PointsMaterial({
    size: 0.055, vertexColors: true, sizeAttenuation: true, transparent: true,
    opacity: 0.95, map: sprite, alphaTest: 0.02,
  }));
  scene.add(points);

  // Хаби — окремим шаром, щоб were помітніші.
  const hubs = cnNodes.filter((n) => n.kind === "hub" || n.kind === "clusterHub" || n.kind === "special");
  const hpos = new Float32Array(hubs.length * 3);
  const hcol = new Float32Array(hubs.length * 3);
  hubs.forEach((n, i) => {
    hpos[i * 3] = n.x; hpos[i * 3 + 1] = n.y; hpos[i * 3 + 2] = n.z;
    c3.set(n.color);
    hcol[i * 3] = c3.r; hcol[i * 3 + 1] = c3.g; hcol[i * 3 + 2] = c3.b;
  });
  const hgeo = new THREE.BufferGeometry();
  hgeo.setAttribute("position", new THREE.BufferAttribute(hpos, 3));
  hgeo.setAttribute("color", new THREE.BufferAttribute(hcol, 3));
  const hubPoints = new THREE.Points(hgeo, new THREE.PointsMaterial({
    size: 0.11, vertexColors: true, sizeAttenuation: true, map: sprite, alphaTest: 0.02,
  }));
  scene.add(hubPoints);

  // Ребра.
  const epos = new Float32Array(cnEdges.length * 6);
  const ecol = new Float32Array(cnEdges.length * 6);
  cnEdges.forEach((e, i) => {
    const a = cnNodes[e.a];
    const b = cnNodes[e.b];
    epos.set([a.x, a.y, a.z, b.x, b.y, b.z], i * 6);
    c3.set(e.color);
    ecol.set([c3.r, c3.g, c3.b, c3.r, c3.g, c3.b], i * 6);
  });
  const egeo = new THREE.BufferGeometry();
  egeo.setAttribute("position", new THREE.BufferAttribute(epos, 3));
  egeo.setAttribute("color", new THREE.BufferAttribute(ecol, 3));
  const edges = new THREE.LineSegments(egeo, new THREE.LineBasicMaterial({
    vertexColors: true, transparent: true, opacity: 0.4,
  }));
  scene.add(edges);

  // Крос-зв'язки.
  const xpos = new Float32Array(cnCross.length * 6);
  const xcol = new Float32Array(cnCross.length * 6);
  cnCross.forEach((c, i) => {
    const a = cnByAddr.get(c.from);
    const b = cnByAddr.get(c.to);
    if (a === undefined || b === undefined) return;
    const na = cnNodes[a];
    const nb = cnNodes[b];
    xpos.set([na.x, na.y, na.z, nb.x, nb.y, nb.z], i * 6);
    const colr = c.hops > 1 ? new THREE.Color(0x64748b) : new THREE.Color(0x38bdf8);
    xcol.set([colr.r, colr.g, colr.b, colr.r, colr.g, colr.b], i * 6);
  });
  const xgeo = new THREE.BufferGeometry();
  xgeo.setAttribute("position", new THREE.BufferAttribute(xpos, 3));
  xgeo.setAttribute("color", new THREE.BufferAttribute(xcol, 3));
  const cross = new THREE.LineSegments(xgeo, new THREE.LineDashedMaterial({
    vertexColors: true, transparent: true, opacity: 0.6, dashSize: 0.12, gapSize: 0.08,
  }));
  cross.computeLineDistances();
  scene.add(cross);

  scene.add(new THREE.AmbientLight(0xffffff, 1.6));

  cnThree = { renderer, scene, camera, controls, points, edges, cross, raf: 0 };

  const resize = (): void => {
    const r = mount.getBoundingClientRect();
    const w = Math.max(1, Math.round(r.width));
    const h = Math.max(1, Math.round(r.height));
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  };
  resize();
  window.addEventListener("resize", resize);

  const tick = (): void => {
    if (!cnThree || !cnOpen) return;
    cnThree.raf = requestAnimationFrame(tick);
    cnThree.controls.update();
    cnThree.renderer.render(cnThree.scene, cnThree.camera);
  };
  tick();
}

/** Кругла текстура для точок — щоб вузли були крапками, а не квадратами. */
function roundSprite(): THREE.Texture {
  const size = 64;
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const g = c.getContext("2d");
  if (g) {
    const grad = g.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
    grad.addColorStop(0, "rgba(255,255,255,1)");
    grad.addColorStop(0.55, "rgba(255,255,255,0.85)");
    grad.addColorStop(1, "rgba(255,255,255,0)");
    g.fillStyle = grad;
    g.fillRect(0, 0, size, size);
  }
  const tex = new THREE.CanvasTexture(c);
  tex.needsUpdate = true;
  return tex;
}

function dispose3D(): void {
  if (!cnThree) return;
  cancelAnimationFrame(cnThree.raf);
  cnThree.controls.dispose();
  cnThree.renderer.dispose();
  cnThree.renderer.domElement.remove();
  cnThree = null;
}

/* ---------- перемикач режимів ---------- */

export function setCapitalNetworkMode(mode: "2d" | "3d"): void {
  cnMode = mode;
  const b2 = $("#cn-mode-2d");
  const b3 = $("#cn-mode-3d");
  b2?.classList.toggle("bg-accent", mode === "2d");
  b2?.classList.toggle("text-white", mode === "2d");
  b3?.classList.toggle("bg-accent", mode === "3d");
  b3?.classList.toggle("text-white", mode === "3d");
  if (mode === "3d") {
    const canvas = $("#cn-canvas");
    canvas?.classList.add("hidden");
    void mount3D();
  } else {
    dispose3D();
    const canvas = $("#cn-canvas");
    canvas?.classList.remove("hidden");
    if (!cnRaf) cnRaf = requestAnimationFrame(loop2D);
  }
}

/* ---------- взаємодія ---------- */

function nodeAt(px: number, py: number): number {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas) return -1;
  const rect = canvas.getBoundingClientRect();
  const W = rect.width;
  const H = rect.height;
  let best = -1;
  let bestD = 18 * 18;
  for (let i = 0; i < cnNodes.length; i++) {
    const [x, y] = toScreen(cnNodes[i].x, cnNodes[i].y, W, H);
    const dx = x - px;
    const dy = y - py;
    const d = dx * dx + dy * dy;
    if (d < bestD) { bestD = d; best = i; }
  }
  return best;
}

function showTooltip(idx: number, px: number, py: number): void {
  const tip = $("#cn-tooltip");
  if (!tip) return;
  if (idx < 0) { tip.classList.add("hidden"); return; }
  const n = cnNodes[idx];
  const rows: string[] = [];
  rows.push(`<div class="font-mono text-slate-100">${n.label || shortAddr(n.addr)}</div>`);
  if (n.label) rows.push(`<div class="font-mono text-slate-400 text-[10px]">${shortAddr(n.addr)}</div>`);
  if (n.groupLabel) rows.push(`<div class="text-slate-300">${n.groupLabel}</div>`);
  if (n.balanceUsd) rows.push(`<div class="text-slate-400">${fmtUsd(n.balanceUsd)}</div>`);
  if (n.tokens?.length) rows.push(`<div class="text-slate-400">${n.tokens.slice(0, 6).join(", ")}</div>`);
  rows.push(`<div class="text-slate-500">${t("cn_degree")}: ${n.degree} · ${t("cn_hops")}: ${n.hops ?? "—"}</div>`);
  tip.innerHTML = rows.join("");
  tip.classList.remove("hidden");
  const modal = $("#capital-modal");
  const r = modal?.getBoundingClientRect();
  const left = Math.min((r?.width ?? 800) - 230, px + 16);
  tip.style.left = `${Math.max(8, left)}px`;
  tip.style.top = `${Math.max(8, py - 12)}px`;
}

function showDetail(idx: number): void {
  const panel = $("#cn-detail");
  if (!panel) return;
  if (idx < 0) { panel.classList.add("translate-x-full"); return; }
  const n = cnNodes[idx];
  const rows: Array<[string, string]> = [
    [t("cn_address"), `<span class="font-mono">${escAttr(n.addr)}</span>`],
  ];
  if (n.label) rows.push([t("cn_label"), escAttr(n.label)]);
  if (n.groupLabel) rows.push([t("cn_group"), escAttr(n.groupLabel)]);
  rows.push([t("cn_role"), escAttr(n.kind)]);
  rows.push([t("cn_degree"), String(n.degree)]);
  rows.push([t("cn_links_inside"), String(n.internalDegree)]);
  rows.push([t("cn_participation"), String(n.participation)]);
  rows.push([t("cn_hops"), n.hops === null ? "—" : String(n.hops)]);
  if (n.balanceUsd) rows.push([t("cn_balance"), fmtUsd(n.balanceUsd)]);
  if (n.tokens?.length) rows.push([t("cn_tokens"), escAttr(n.tokens.join(", "))]);
  panel.innerHTML = `
    <div class="flex items-start justify-between gap-2 mb-2">
      <h3 class="font-semibold text-sm">${n.label || shortAddr(n.addr)}</h3>
      <button type="button" id="cn-detail-close" class="text-slate-400 hover:text-slate-200 text-xl leading-none">×</button>
    </div>
    <div class="space-y-1 text-xs">
      ${rows.map(([k, v]) => `<div class="flex justify-between gap-3 border-b border-edge/60 pb-1"><span class="text-slate-500">${k}</span><span class="text-slate-200 text-right">${v}</span></div>`).join("")}
    </div>
    <button type="button" data-cn-graph="${escAttr(n.addr)}"
      class="mt-3 w-full text-xs px-2 py-1.5 rounded-lg bg-accent/15 text-accent border border-accent/30 hover:bg-accent/25 transition">${t("cn_open_graph")}</button>`;
  panel.classList.remove("translate-x-full");
  panel.querySelector("#cn-detail-close")?.addEventListener("click", () => panel.classList.add("translate-x-full"));
  panel.querySelector("[data-cn-graph]")?.addEventListener("click", (ev) => {
    const addr = (ev.currentTarget as HTMLElement).getAttribute("data-cn-graph");
    if (addr) openGraphForWallet(addr);
  });
}

/** Вмикає обробники один раз (модалка може відкриватися повторно). */
export function setupCapitalNetworkInteractions(): void {
  const canvas = $<HTMLCanvasElement>("#cn-canvas");
  if (!canvas || canvas.dataset.cnBound === "1") return;
  canvas.dataset.cnBound = "1";

  canvas.addEventListener("wheel", (ev) => {
    ev.preventDefault();
    const f = ev.deltaY < 0 ? 1.12 : 1 / 1.12;
    cnView.scale = Math.min(600, Math.max(0.05, cnView.scale * f));
  }, { passive: false });

  canvas.addEventListener("pointerdown", (ev) => {
    cnPan = { x: ev.clientX, y: ev.clientY, ox: cnView.ox, oy: cnView.oy };
    canvas.setPointerCapture(ev.pointerId);
  });
  canvas.addEventListener("pointermove", (ev) => {
    const rect = canvas.getBoundingClientRect();
    const px = ev.clientX - rect.left;
    const py = ev.clientY - rect.top;
    cnPointer = { x: px, y: py, inside: true };
    if (cnPan) {
      cnView.ox = cnPan.ox + (ev.clientX - cnPan.x);
      cnView.oy = cnPan.oy - (ev.clientY - cnPan.y);
      return;
    }
    const idx = nodeAt(px, py);
    cnHoverIdx = idx;
    showTooltip(idx, px, py);
  });
  const endPan = (ev: PointerEvent): void => {
    if (cnPan) {
      const moved = Math.abs(ev.clientX - cnPan.x) + Math.abs(ev.clientY - cnPan.y) > 4;
      cnPan = null;
      if (!moved) {
        const rect = canvas.getBoundingClientRect();
        const idx = nodeAt(ev.clientX - rect.left, ev.clientY - rect.top);
        showDetail(idx);
      }
    }
  };
  canvas.addEventListener("pointerup", endPan);
  canvas.addEventListener("pointercancel", endPan);
  canvas.addEventListener("pointerleave", () => {
    cnHoverIdx = -1;
    cnPointer.inside = false;
    showTooltip(-1, 0, 0);
  });
  canvas.addEventListener("dblclick", (ev) => {
    const rect = canvas.getBoundingClientRect();
    const idx = nodeAt(ev.clientX - rect.left, ev.clientY - rect.top);
    if (idx >= 0) openGraphForWallet(cnNodes[idx].addr);
  });
}
