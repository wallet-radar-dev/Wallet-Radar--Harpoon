/* ---------- Спільні UI-примітиви (без залежностей від фіч) ---------- */

import { $ } from "./util";

/** Тимчасове сповіщення в кутку (само зникає). */
export function toast(title: string, body: string, cls = "border-accent"): void {
  const wrap = $("#toasts");
  const el = document.createElement("div");
  el.className = `bg-panel border ${cls} rounded-xl p-3 shadow-lg animate-slidein`;
  el.innerHTML = `<p class="text-sm font-medium">${title}</p><p class="text-xs text-slate-400 mt-0.5">${body}</p>`;
  wrap.appendChild(el);
  setTimeout(() => {
    el.style.transition = "opacity .4s";
    el.style.opacity = "0";
    setTimeout(() => el.remove(), 400);
  }, 4200);
}

/** Екранування для вставки рядка в HTML-атрибут (data-*, title тощо). */
export const escAttr = (s: string): string => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

/** Зовнішнє посилання (нова вкладка, noopener). */
export const link = (href: string, text: string): string =>
  `<a href="${href}" target="_blank" rel="noopener" class="text-accent hover:underline break-all">${text}</a>`;

/** Рядок «мітка → значення» для таблиць деталей (dl/grid). */
export const row = (label: string, valueHtml: string): string => `
  <div class="grid grid-cols-3 gap-3 py-2 border-b border-edge/60">
    <dt class="text-xs text-slate-400 col-span-1">${label}</dt>
    <dd class="text-sm col-span-2 break-all">${valueHtml}</dd>
  </div>`;
