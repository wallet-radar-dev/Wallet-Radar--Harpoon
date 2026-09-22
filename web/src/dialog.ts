/* ---------- Доступність модалок: фокус усередину, трап, повернення ---------- */
// Modals are opened/closed by toggling `hidden` in many places, so rather than
// touch every call site we observe the [role="dialog"] elements: on open we move
// focus in and trap Tab within the dialog; on close we restore focus to the
// element that opened it (WCAG 2.4.3 / dialog pattern). Escape-to-close already
// exists globally.
export function setupDialogFocusManagement(): void {
  const SEL =
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
  let lastTrigger: HTMLElement | null = null;
  let trap: ((e: KeyboardEvent) => void) | null = null;

  const visibleFocusables = (dialog: HTMLElement): HTMLElement[] =>
    Array.from(dialog.querySelectorAll<HTMLElement>(SEL)).filter((el) => el.offsetParent !== null);

  const removeTrap = (): void => {
    if (trap) {
      document.removeEventListener("keydown", trap, true);
      trap = null;
    }
  };

  // The interactive background regions to freeze while a dialog is open, so
  // screen readers and Tab cannot reach them (the backdrop already dims them).
  const background = (): HTMLElement[] =>
    [document.querySelector<HTMLElement>("header"), document.getElementById("main")].filter((el): el is HTMLElement => !!el);
  const setBackgroundInert = (on: boolean): void => {
    background().forEach((el) => {
      if (on) el.setAttribute("inert", "");
      else el.removeAttribute("inert");
    });
  };

  const onOpen = (dialog: HTMLElement): void => {
    removeTrap();
    setBackgroundInert(true);
    lastTrigger = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => {
      const items = visibleFocusables(dialog);
      (items[0] ?? dialog).focus();
    });
    trap = (e: KeyboardEvent): void => {
      if (e.key !== "Tab") return;
      const items = visibleFocusables(dialog);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement as HTMLElement;
      if (!dialog.contains(active)) {
        e.preventDefault();
        first.focus();
      } else if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", trap, true);
  };

  const onClose = (): void => {
    removeTrap();
    setBackgroundInert(false);
    lastTrigger?.focus?.();
    lastTrigger = null;
  };

  document.querySelectorAll<HTMLElement>('[role="dialog"]').forEach((dialog) => {
    let open = !dialog.classList.contains("hidden");
    new MutationObserver(() => {
      const nowOpen = !dialog.classList.contains("hidden");
      if (nowOpen === open) return;
      open = nowOpen;
      if (nowOpen) onOpen(dialog);
      else onClose();
    }).observe(dialog, { attributes: true, attributeFilter: ["class"] });
  });
}
