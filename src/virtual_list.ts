import { computeVirtualGridWindow, computeVirtualWindow } from "./workflow";

export interface VirtualListOptions<T> {
  container: HTMLElement;
  itemHeight: number;
  gap?: number;
  overscan?: number;
  emptyElement?: () => HTMLElement;
  renderItem: (item: T, index: number) => HTMLElement;
  getKey?: (item: T, index: number) => string;
}

export class VirtualListController<T> {
  private readonly container: HTMLElement;
  private readonly baseItemHeight: number;
  private measuredStride: number;
  private readonly gap: number;
  private readonly overscan: number;
  private readonly emptyElement?: () => HTMLElement;
  private readonly renderItem: (item: T, index: number) => HTMLElement;
  private readonly getKey?: (item: T, index: number) => string;

  private readonly topSpacer: HTMLDivElement;
  private readonly sliceEl: HTMLDivElement;
  private readonly bottomSpacer: HTMLDivElement;

  private items: readonly T[] = [];
  private lastStart = -1;
  private lastEnd = -1;
  private rafPending = false;
  private mounted = false;
  private resizeObserver: ResizeObserver | null = null;

  constructor(opts: VirtualListOptions<T>) {
    this.container = opts.container;
    this.baseItemHeight = Math.max(24, opts.itemHeight);
    this.gap = Math.max(0, opts.gap ?? 0);
    this.measuredStride = this.baseItemHeight + this.gap;
    this.overscan = Math.max(2, opts.overscan ?? 6);
    this.emptyElement = opts.emptyElement;
    this.renderItem = opts.renderItem;
    this.getKey = opts.getKey;

    this.topSpacer = document.createElement("div");
    this.topSpacer.className = "virtual-spacer";
    this.sliceEl = document.createElement("div");
    this.sliceEl.className = "col-flex";
    this.sliceEl.style.gap = `${this.gap}px`;
    this.sliceEl.style.flex = "0 0 auto";
    this.bottomSpacer = document.createElement("div");
    this.bottomSpacer.className = "virtual-spacer";

    this.container.addEventListener("scroll", this.onScroll, { passive: true });
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => {
        if (this.items.length > 0 && this.mounted) {
          this.scheduleRender(false);
        }
      });
      this.resizeObserver.observe(this.container);
    }
  }

  private ensureShell(): void {
    if (
      this.mounted &&
      this.topSpacer.parentElement === this.container &&
      this.sliceEl.parentElement === this.container &&
      this.bottomSpacer.parentElement === this.container
    ) {
      return;
    }
    this.container.style.gap = "0px";
    this.container.replaceChildren(this.topSpacer, this.sliceEl, this.bottomSpacer);
    this.mounted = true;
  }

  private onScroll = (): void => {
    this.scheduleRender(false);
  };

  private scheduleRender(force: boolean): void {
    if (force) {
      this.renderNow(true);
      return;
    }
    if (this.rafPending) return;
    this.rafPending = true;
    requestAnimationFrame(() => {
      this.rafPending = false;
      this.renderNow(false);
    });
  }

  setItems(items: readonly T[], opts?: { resetScroll?: boolean; force?: boolean }): void {
    this.items = items;
    if (opts?.resetScroll) {
      this.container.scrollTop = 0;
    }
    this.lastStart = -1;
    this.lastEnd = -1;
    if (items.length === 0) {
      this.mounted = false;
      if (this.emptyElement) {
        this.container.replaceChildren(this.emptyElement());
      } else {
        this.container.replaceChildren();
      }
      return;
    }
    this.ensureShell();
    this.renderNow(opts?.force ?? true);
  }

  scrollToIndex(index: number, align: "nearest" | "center" = "nearest"): void {
    if (index < 0 || index >= this.items.length) return;
    const viewport = this.container.clientHeight || this.measuredStride * 6;
    const itemTop = index * this.measuredStride;
    const itemBottom = itemTop + this.measuredStride;
    const currentTop = this.container.scrollTop;
    const currentBottom = currentTop + viewport;

    if (align === "center") {
      this.container.scrollTop = Math.max(0, itemTop - (viewport - this.measuredStride) / 2);
    } else if (itemTop < currentTop) {
      this.container.scrollTop = itemTop;
    } else if (itemBottom > currentBottom) {
      this.container.scrollTop = Math.max(0, itemBottom - viewport);
    }
    this.lastStart = -1;
    this.lastEnd = -1;
    this.renderNow(true);
  }

  getRenderedRange(): { start: number; end: number } {
    return { start: Math.max(0, this.lastStart), end: Math.max(0, this.lastEnd) };
  }

  private renderNow(force: boolean): void {
    if (this.items.length === 0) return;
    this.ensureShell();

    const viewport = this.container.clientHeight || this.measuredStride * 8;
    const win = computeVirtualWindow({
      total: this.items.length,
      itemHeight: this.measuredStride,
      viewportHeight: viewport,
      scrollTop: this.container.scrollTop,
      overscan: this.overscan,
    });

    if (!force && win.start === this.lastStart && win.end === this.lastEnd) {
      return;
    }

    // Preserve active input focus + caret when scrolling/re-rendering visible rows.
    const active = document.activeElement as HTMLElement | null;
    let activeKey: string | null = null;
    let activeRole: string | null = null;
    let selStart: number | null = null;
    let selEnd: number | null = null;
    if (active && this.sliceEl.contains(active)) {
      const host = active.closest("[data-vkey]") as HTMLElement | null;
      activeKey = host?.dataset.vkey ?? null;
      activeRole = active.getAttribute("data-vfocus") || active.tagName.toLowerCase();
      if (active instanceof HTMLInputElement && active.type !== "checkbox") {
        selStart = active.selectionStart;
        selEnd = active.selectionEnd;
      }
    }

    this.topSpacer.style.height = `${win.topSpacer}px`;
    this.bottomSpacer.style.height = `${win.bottomSpacer}px`;

    const frag = document.createDocumentFragment();
    for (let i = win.start; i < win.end; i += 1) {
      const item = this.items[i];
      const node = this.renderItem(item, i);
      node.dataset.vindex = String(i);
      if (this.getKey) {
        node.dataset.vkey = this.getKey(item, i);
      }
      frag.appendChild(node);
    }
    this.sliceEl.replaceChildren(frag);
    this.lastStart = win.start;
    this.lastEnd = win.end;

    // Measure actual row height once visible to refine spacer calibration.
    const first = this.sliceEl.firstElementChild as HTMLElement | null;
    if (first && this.container.clientHeight > 0) {
      const rect = first.getBoundingClientRect();
      if (rect.height >= 24 && Math.abs(rect.height + this.gap - this.measuredStride) >= 2) {
        const clamped = Math.max(this.baseItemHeight * 0.7, Math.min(this.baseItemHeight * 1.8, rect.height));
        const nextStride = Math.round(clamped) + this.gap;
        if (Math.abs(nextStride - this.measuredStride) >= 2) {
          this.measuredStride = nextStride;
          const recalc = computeVirtualWindow({
            total: this.items.length,
            itemHeight: this.measuredStride,
            viewportHeight: viewport,
            scrollTop: this.container.scrollTop,
            overscan: this.overscan,
          });
          this.topSpacer.style.height = `${recalc.topSpacer}px`;
          this.bottomSpacer.style.height = `${recalc.bottomSpacer}px`;
        }
      }
    }

    if (activeKey) {
      const targetRow = Array.from(this.sliceEl.children).find(
        (el): el is HTMLElement => el instanceof HTMLElement && el.dataset.vkey === activeKey
      );
      if (targetRow) {
        const focusEl = (
          activeRole && activeRole !== "input"
            ? targetRow.querySelector(`[data-vfocus="${activeRole}"]`)
            : targetRow.querySelector(activeRole || "input")
        ) as HTMLElement | null;
        if (focusEl) {
          focusEl.focus({ preventScroll: true });
          if (focusEl instanceof HTMLInputElement && selStart != null && selEnd != null) {
            try {
              focusEl.setSelectionRange(selStart, selEnd);
            } catch {
              // ignore non-text inputs
            }
          }
        }
      }
    }
  }

  destroy(): void {
    this.container.removeEventListener("scroll", this.onScroll);
    this.resizeObserver?.disconnect();
  }
}

export interface VirtualGridOptions<T> {
  container: HTMLElement;
  minColWidth: number;
  rowHeight: number;
  gap?: number;
  overscanRows?: number;
  emptyElement?: () => HTMLElement;
  renderItem: (item: T, index: number) => HTMLElement;
}

export class VirtualGridController<T> {
  private readonly container: HTMLElement;
  private readonly minColWidth: number;
  private readonly rowHeight: number;
  private readonly gap: number;
  private readonly overscanRows: number;
  private readonly emptyElement?: () => HTMLElement;
  private readonly renderItem: (item: T, index: number) => HTMLElement;

  private readonly topSpacer: HTMLDivElement;
  private readonly gridEl: HTMLDivElement;
  private readonly bottomSpacer: HTMLDivElement;

  private items: readonly T[] = [];
  private lastStart = -1;
  private lastEnd = -1;
  private lastCols = 0;
  private rafPending = false;
  private mounted = false;
  private resizeObserver: ResizeObserver | null = null;

  constructor(opts: VirtualGridOptions<T>) {
    this.container = opts.container;
    this.minColWidth = Math.max(80, opts.minColWidth);
    this.rowHeight = Math.max(60, opts.rowHeight);
    this.gap = Math.max(0, opts.gap ?? 8);
    this.overscanRows = Math.max(1, opts.overscanRows ?? 2);
    this.emptyElement = opts.emptyElement;
    this.renderItem = opts.renderItem;

    this.topSpacer = document.createElement("div");
    this.topSpacer.className = "virtual-spacer";
    this.gridEl = document.createElement("div");
    this.gridEl.className = "virtual-grid-slice";
    this.gridEl.style.display = "grid";
    this.gridEl.style.gap = `${this.gap}px`;
    this.bottomSpacer = document.createElement("div");
    this.bottomSpacer.className = "virtual-spacer";

    this.container.addEventListener("scroll", this.onScroll, { passive: true });
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => {
        if (this.items.length > 0 && this.mounted) {
          this.scheduleRender(false);
        }
      });
      this.resizeObserver.observe(this.container);
    }
  }

  private ensureShell(): void {
    if (
      this.mounted &&
      this.topSpacer.parentElement === this.container &&
      this.gridEl.parentElement === this.container &&
      this.bottomSpacer.parentElement === this.container
    ) {
      return;
    }
    this.container.style.display = "block";
    this.container.replaceChildren(this.topSpacer, this.gridEl, this.bottomSpacer);
    this.mounted = true;
  }

  private onScroll = (): void => {
    this.scheduleRender(false);
  };

  private scheduleRender(force: boolean): void {
    if (force) {
      this.renderNow(true);
      return;
    }
    if (this.rafPending) return;
    this.rafPending = true;
    requestAnimationFrame(() => {
      this.rafPending = false;
      this.renderNow(false);
    });
  }

  setItems(items: readonly T[], opts?: { resetScroll?: boolean; force?: boolean }): void {
    this.items = items;
    if (opts?.resetScroll) {
      this.container.scrollTop = 0;
    }
    this.lastStart = -1;
    this.lastEnd = -1;
    if (items.length === 0) {
      this.mounted = false;
      if (this.emptyElement) {
        this.container.replaceChildren(this.emptyElement());
      } else {
        this.container.replaceChildren();
      }
      return;
    }
    this.ensureShell();
    this.renderNow(opts?.force ?? true);
  }

  scrollToIndex(index: number): void {
    if (index < 0 || index >= this.items.length) return;
    const cols = Math.max(1, this.lastCols || 4);
    const row = Math.floor(index / cols);
    const rowTop = row * this.rowHeight;
    const rowBottom = rowTop + this.rowHeight;
    const viewport = this.container.clientHeight || this.rowHeight * 3;
    if (rowTop < this.container.scrollTop) {
      this.container.scrollTop = rowTop;
    } else if (rowBottom > this.container.scrollTop + viewport) {
      this.container.scrollTop = Math.max(0, rowBottom - viewport);
    }
    this.lastStart = -1;
    this.renderNow(true);
  }

  private renderNow(force: boolean): void {
    if (this.items.length === 0) return;
    this.ensureShell();

    const width = this.container.clientWidth || this.minColWidth * 4;
    const viewport = this.container.clientHeight || this.rowHeight * 3;
    const win = computeVirtualGridWindow({
      total: this.items.length,
      containerWidth: width,
      minColWidth: this.minColWidth,
      rowHeight: this.rowHeight,
      gap: this.gap,
      viewportHeight: viewport,
      scrollTop: this.container.scrollTop,
      overscanRows: this.overscanRows,
    });

    if (!force && win.start === this.lastStart && win.end === this.lastEnd && win.cols === this.lastCols) {
      return;
    }

    this.gridEl.style.gridTemplateColumns = `repeat(${win.cols}, minmax(0, 1fr))`;
    this.topSpacer.style.height = `${win.topSpacer}px`;
    this.bottomSpacer.style.height = `${win.bottomSpacer}px`;

    const frag = document.createDocumentFragment();
    for (let i = win.start; i < win.end; i += 1) {
      frag.appendChild(this.renderItem(this.items[i], i));
    }
    this.gridEl.replaceChildren(frag);
    this.lastCols = win.cols;
    this.lastStart = win.start;
    this.lastEnd = win.end;
  }

  destroy(): void {
    this.container.removeEventListener("scroll", this.onScroll);
    this.resizeObserver?.disconnect();
  }
}

export interface VirtualHorizontalOptions<T> {
  container: HTMLElement;
  itemWidth: number;
  gap?: number;
  overscan?: number;
  renderItem: (item: T, index: number) => HTMLElement;
}

export class VirtualHorizontalController<T> {
  private readonly container: HTMLElement;
  private readonly stride: number;
  private readonly gap: number;
  private readonly overscan: number;
  private readonly renderItem: (item: T, index: number) => HTMLElement;

  private readonly leftSpacer: HTMLDivElement;
  private readonly sliceEl: HTMLDivElement;
  private readonly rightSpacer: HTMLDivElement;

  private items: readonly T[] = [];
  private lastStart = -1;
  private lastEnd = -1;
  private rafPending = false;
  private mounted = false;

  constructor(opts: VirtualHorizontalOptions<T>) {
    this.container = opts.container;
    this.gap = Math.max(0, opts.gap ?? 6);
    this.stride = Math.max(40, opts.itemWidth) + this.gap;
    this.overscan = Math.max(3, opts.overscan ?? 6);
    this.renderItem = opts.renderItem;

    this.leftSpacer = document.createElement("div");
    this.leftSpacer.className = "virtual-hspacer";
    this.sliceEl = document.createElement("div");
    this.sliceEl.style.display = "flex";
    this.sliceEl.style.alignItems = "center";
    this.sliceEl.style.flex = "0 0 auto";
    this.sliceEl.style.gap = `${this.gap}px`;
    this.rightSpacer = document.createElement("div");
    this.rightSpacer.className = "virtual-hspacer";

    this.container.addEventListener("scroll", this.onScroll, { passive: true });
  }

  private ensureShell(): void {
    if (
      this.mounted &&
      this.leftSpacer.parentElement === this.container &&
      this.sliceEl.parentElement === this.container &&
      this.rightSpacer.parentElement === this.container
    ) {
      return;
    }
    this.container.style.gap = "0px";
    this.container.replaceChildren(this.leftSpacer, this.sliceEl, this.rightSpacer);
    this.mounted = true;
  }

  private onScroll = (): void => {
    if (this.rafPending) return;
    this.rafPending = true;
    requestAnimationFrame(() => {
      this.rafPending = false;
      this.renderNow(false);
    });
  };

  setItems(items: readonly T[], opts?: { resetScroll?: boolean; force?: boolean }): void {
    this.items = items;
    if (opts?.resetScroll) {
      this.container.scrollLeft = 0;
    }
    this.lastStart = -1;
    this.lastEnd = -1;
    if (items.length === 0) {
      this.mounted = false;
      this.container.replaceChildren();
      return;
    }
    this.ensureShell();
    this.renderNow(opts?.force ?? true);
  }

  scrollToIndex(index: number): void {
    if (index < 0 || index >= this.items.length) return;
    const viewport = this.container.clientWidth || this.stride * 8;
    const targetLeft = index * this.stride;
    const targetRight = targetLeft + this.stride;
    if (targetLeft < this.container.scrollLeft || targetRight > this.container.scrollLeft + viewport) {
      this.container.scrollLeft = Math.max(0, targetLeft - (viewport - this.stride) / 2);
    }
    this.lastStart = -1;
    this.renderNow(true);
  }

  private renderNow(force: boolean): void {
    if (this.items.length === 0) return;
    this.ensureShell();

    const viewport = this.container.clientWidth || this.stride * 8;
    const win = computeVirtualWindow({
      total: this.items.length,
      itemHeight: this.stride,
      viewportHeight: viewport,
      scrollTop: this.container.scrollLeft,
      overscan: this.overscan,
    });

    if (!force && win.start === this.lastStart && win.end === this.lastEnd) {
      return;
    }

    this.leftSpacer.style.width = `${win.topSpacer}px`;
    this.rightSpacer.style.width = `${win.bottomSpacer}px`;

    const frag = document.createDocumentFragment();
    for (let i = win.start; i < win.end; i += 1) {
      frag.appendChild(this.renderItem(this.items[i], i));
    }
    this.sliceEl.replaceChildren(frag);
    this.lastStart = win.start;
    this.lastEnd = win.end;
  }

  destroy(): void {
    this.container.removeEventListener("scroll", this.onScroll);
  }
}

export interface ProgressiveListOptions<T> {
  container: HTMLElement;
  batchSize?: number;
  emptyElement?: () => HTMLElement;
  renderItem: (item: T, index: number) => HTMLElement;
}

export class ProgressiveListController<T> {
  private readonly container: HTMLElement;
  private readonly batchSize: number;
  private readonly emptyElement?: () => HTMLElement;
  private readonly renderItem: (item: T, index: number) => HTMLElement;
  private items: readonly T[] = [];
  private renderedCount = 0;
  private sentinel: HTMLDivElement | null = null;
  private observer: IntersectionObserver | null = null;

  constructor(opts: ProgressiveListOptions<T>) {
    this.container = opts.container;
    this.batchSize = Math.max(8, opts.batchSize ?? 24);
    this.emptyElement = opts.emptyElement;
    this.renderItem = opts.renderItem;
  }

  setItems(items: readonly T[]): void {
    this.observer?.disconnect();
    this.items = items;
    this.renderedCount = 0;
    this.sentinel = null;
    this.container.replaceChildren();
    if (items.length === 0) {
      if (this.emptyElement) {
        this.container.appendChild(this.emptyElement());
      }
      return;
    }
    this.appendNextBatch();
  }

  private appendNextBatch(): void {
    if (this.renderedCount >= this.items.length) return;
    if (this.sentinel) {
      this.sentinel.remove();
      this.sentinel = null;
    }
    const nextEnd = Math.min(this.items.length, this.renderedCount + this.batchSize);
    const frag = document.createDocumentFragment();
    for (let i = this.renderedCount; i < nextEnd; i += 1) {
      frag.appendChild(this.renderItem(this.items[i], i));
    }
    this.renderedCount = nextEnd;
    this.container.appendChild(frag);

    if (this.renderedCount < this.items.length) {
      const sentinel = document.createElement("div");
      sentinel.className = "muted";
      sentinel.style.padding = "8px";
      sentinel.style.textAlign = "center";
      sentinel.style.fontSize = "13px";
      sentinel.textContent = `已加载 ${this.renderedCount}/${this.items.length}，向下滚动自动加载更多…`;
      this.sentinel = sentinel;
      this.container.appendChild(sentinel);
      if (typeof IntersectionObserver !== "undefined") {
        this.observer = new IntersectionObserver(
          (entries) => {
            if (entries.some((e) => e.isIntersecting)) {
              this.observer?.disconnect();
              this.appendNextBatch();
            }
          },
          { root: this.container, rootMargin: "120px 0px" }
        );
        this.observer.observe(sentinel);
      } else {
        this.appendNextBatch();
      }
    }
  }
}
