import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { MutableRefObject, RefObject } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { ConversationTurnView } from "./conversationTurns";
import type { RenderViewState } from "@/components/ChatRenderBlock/useRenderState";
import { retainConversationTurns } from "./turnResidency";
import { followAfterScroll } from "./viewportFollow";

type Anchor = { turnKey: string; messageId?: string; offset: number };
type SavedView = RenderViewState & {
  sizes: Map<string, number>;
  retained: Set<string>;
  scrollTop: number;
  followLatest: boolean;
  anchor?: Anchor;
  width?: number;
  height?: number;
};
export type ConversationViewportController = {
  navigate: (direction: "top" | "up" | "down" | "bottom") => void;
  reveal: (key: string, messageId?: string) => void;
  follow: () => void;
  beforePrepend: () => void;
};
const savedViews = new Map<string, SavedView>();
function viewFor(key: string): SavedView {
  const saved = savedViews.get(key) ?? {
    values: new Map(),
    protectedScopes: new Map(),
    sizes: new Map(),
    retained: new Set(),
    scrollTop: 0,
    followLatest: true,
  };
  savedViews.delete(key);
  savedViews.set(key, saved);
  while (savedViews.size > 8)
    savedViews.delete(savedViews.keys().next().value!);
  return saved;
}
export function __resetConversationViewportForTests() {
  savedViews.clear();
}

export function useConversationViewport({
  turns,
  viewKey,
  scrollRef,
  controllerRef,
  activeKeys,
  mobile,
  onScrollState,
  onTop,
  initialScroll,
}: {
  turns: ConversationTurnView[];
  viewKey: string;
  scrollRef?: RefObject<HTMLDivElement | null>;
  controllerRef?: MutableRefObject<ConversationViewportController | null>;
  activeKeys: ReadonlySet<string>;
  mobile: boolean;
  onScrollState?: (
    following: boolean,
    scrolling: boolean,
    scrollTop: number,
  ) => void;
  onTop?: () => void;
  initialScroll?: { scrollTop?: number; followLatest?: boolean } | null;
}) {
  const initialScrollRef = useRef(initialScroll);
  const view = useMemo(() => {
    const initialScroll = initialScrollRef.current;
    const existing = savedViews.has(viewKey);
    const entry = scrollRef
      ? viewFor(viewKey)
      : ({
          values: new Map(),
          protectedScopes: new Map(),
          sizes: new Map(),
          retained: new Set<string>(),
          scrollTop: 0,
          followLatest: true,
        } as SavedView);
    if (!existing && initialScroll) {
      entry.scrollTop = initialScroll.scrollTop ?? 0;
      entry.followLatest = initialScroll.followLatest ?? true;
    }
    return entry;
  }, [viewKey, scrollRef]);
  const [revision, refresh] = useState(0);
  const [ready, setReady] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const restored = useRef(false);
  const requestRef = useRef<{
    key: string;
    messageId?: string;
    align: "start" | "end";
    offset?: number;
    behavior?: ScrollBehavior;
  } | null>(null);
  const programmatic = useRef<number | null>(null);
  const direction = useRef(0);
  const lastScroll = useRef(view.scrollTop);
  const pointerActive = useRef(false);
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const prependAnchor = useRef<Anchor | null>(null);
  const keysRef = useRef<string[]>([]);
  if (
    keysRef.current.length !== turns.length ||
    turns.some((turn, index) => keysRef.current[index] !== turn.key)
  ) {
    keysRef.current = turns.map((turn) => turn.key);
  }
  const keys = keysRef.current;
  const keyIndex = useMemo(
    () => new Map(keys.map((key, index) => [key, index])),
    [keys],
  );
  const latest = useRef({
    turns,
    keys,
    keyIndex,
    onScrollState,
    onTop,
    activeKeys,
  });
  latest.current = { turns, keys, keyIndex, onScrollState, onTop, activeKeys };
  const high = mobile ? 192 : 256;
  const low = mobile ? 128 : 192;
  const buffer = 12;
  const getScroller = useCallback(
    () => scrollRef?.current ?? null,
    [scrollRef],
  );
  const enabled = Boolean(scrollRef);
  const initialRetained = useRef(view.retained.size > 0);
  if (!initialRetained.current && view.retained.size === 0) {
    const initial = view.followLatest ? keys.slice(-high) : keys.slice(0, high);
    for (const key of initial) view.retained.add(key);
  }
  const readProtected = useCallback(() => {
    const keep = new Set(latest.current.activeKeys);
    for (const key of view.protectedScopes.keys()) keep.add(key);
    if (requestRef.current) keep.add(requestRef.current.key);
    const scroller = getScroller();
    const focus = document.activeElement?.closest<HTMLElement>(
      "[data-rua-turn-key]",
    );
    if (focus && scroller?.contains(focus))
      keep.add(focus.dataset.ruaTurnKey!);
    const selection = window.getSelection();
    if (
      selection &&
      !selection.isCollapsed &&
      selection.rangeCount &&
      scroller
    ) {
      const range = selection.getRangeAt(0);
      for (const row of scroller.querySelectorAll<HTMLElement>(
        "[data-rua-turn-key]",
      )) {
        if (range.intersectsNode(row)) keep.add(row.dataset.ruaTurnKey!);
      }
    }
    return keep;
  }, [getScroller, view]);
  const rangeExtractor = useCallback(
    (range: { startIndex: number; endIndex: number }) => {
      void revision; // Retention changes invalidate the virtualizer's range memo.
      const all = latest.current.keys;
      if (!enabled || all.length <= high) return all.map((_, index) => index);
      const required = readProtected();
      if (view.followLatest) {
        for (const key of all.slice(-2)) required.add(key);
      } else {
        // Visible destination is immediate; nearby whole turns restore gradually.
        for (
          let i = Math.max(0, range.startIndex - 1);
          i <= Math.min(all.length - 1, range.endIndex + 1);
          i++
        )
          required.add(all[i]);
      }
      // A just-read group joins the retained history until the next budget
      // reclamation, even if the reader immediately reverses direction.
      for (const key of required) view.retained.add(key);
      const retained = view.retained;
      return [...retained]
        .map((key) => latest.current.keyIndex.get(key))
        .filter((index): index is number => index !== undefined)
        .sort((a, b) => a - b);
    },
    [enabled, high, readProtected, view, revision],
  );
  const virtualizer = useVirtualizer({
    count: keys.length,
    getScrollElement: getScroller,
    getItemKey: useCallback((index: number) => keys[index], [keys]),
    estimateSize: useCallback(
      (index: number) => view.sizes.get(keys[index]) ?? 400,
      [keys, view],
    ),
    rangeExtractor,
    enabled,
    useFlushSync: false,
    initialOffset: () => view.scrollTop,
    paddingStart: 24,
    paddingEnd: 8,
    gap: 0,
    anchorTo: "start",
    followOnAppend: false,
    measureElement: (element, entry) => {
      const border = entry?.borderBoxSize?.[0]?.blockSize;
      const height = border ?? element.getBoundingClientRect().height;
      const key = element.getAttribute("data-rua-turn-key");
      if (key && height > 0) view.sizes.set(key, height);
      return height || (key ? view.sizes.get(key) : undefined) || 400;
    },
  });
  // This controller owns reading-anchor compensation; the low-level virtualizer
  // must not apply the same height delta again after our layout effect.
  virtualizer.shouldAdjustScrollPositionOnItemSizeChange = () => false;
  const virtRef = useRef(virtualizer);
  virtRef.current = virtualizer;
  const writeScroll = useCallback(
    (top: number, behavior: ScrollBehavior = "auto") => {
      const node = getScroller();
      if (!node) return;
      const previous = node.scrollTop;
      if (typeof node.scrollTo === "function") node.scrollTo({ top, behavior });
      else node.scrollTop = top;
      if (node.scrollTop !== previous) programmatic.current = node.scrollTop;
      view.scrollTop = node.scrollTop;
      lastScroll.current = node.scrollTop;
    },
    [getScroller, view],
  );
  const capture = useCallback((): Anchor | undefined => {
    const node = getScroller();
    if (!node) return;
    const top = node.getBoundingClientRect().top;
    const item = virtRef.current.getVirtualItemForOffset(node.scrollTop);
    let row = item ? virtRef.current.elementsCache.get(item.key) : undefined;
    const intersectsTop = (element: Element) => {
      const rect = element.getBoundingClientRect();
      return rect.bottom > top + 1 && rect.top < top + node.clientHeight;
    };
    // Width changes and newly restored turns can make the estimated index
    // briefly stale. Resolve against the bounded mounted DOM in that case.
    if (!row || !intersectsTop(row))
      row = [
        ...node.querySelectorAll<HTMLElement>("[data-rua-turn-key]"),
      ].find(intersectsTop);
    if (!row) return;
    const message = [
      ...row.querySelectorAll<HTMLElement>("[data-rua-chat-message-id]"),
    ].find((message) => message.getBoundingClientRect().bottom > top + 1);
    const element = message ?? row;
    return {
      turnKey: row.getAttribute("data-rua-turn-key")!,
      messageId: message?.dataset.ruaChatMessageId,
      offset: element.getBoundingClientRect().top - top,
    };
  }, [getScroller]);
  const preserve = useCallback(
    (anchor: Anchor) => {
      const node = getScroller();
      if (!node) return false;
      const row = virtRef.current.elementsCache.get(anchor.turnKey);
      const message = anchor.messageId
        ? [
            ...(row?.querySelectorAll<HTMLElement>(
              "[data-rua-chat-message-id]",
            ) ?? []),
          ].find(
            (element) =>
              element.dataset.ruaChatMessageId === anchor.messageId,
          )
        : undefined;
      const target = message ?? row;
      if (!target) return false;
      const delta =
        target.getBoundingClientRect().top -
        node.getBoundingClientRect().top -
        anchor.offset;
      if (Math.abs(delta) > 0.5) writeScroll(node.scrollTop + delta);
      return true;
    },
    [getScroller, writeScroll],
  );
  const follow = useCallback(
    (behavior: ScrollBehavior = "auto") => {
      view.followLatest = true;
      view.anchor = undefined;
      const lastKey = latest.current.keys[latest.current.keys.length - 1];
      if (lastKey) {
        view.retained.add(lastKey);
        requestRef.current = { key: lastKey, align: "end", behavior };
      }
      const node = getScroller();
      if (node) writeScroll(node.scrollHeight, behavior);
      latest.current.onScrollState?.(true, false, node?.scrollTop ?? 0);
      refresh((value) => value + 1);
    },
    [getScroller, view, writeScroll],
  );
  const reveal = useCallback(
    (key: string, messageId?: string) => {
      if (!latest.current.keyIndex.has(key)) return;
      view.followLatest = false;
      view.retained.add(key);
      view.anchor = { turnKey: key, messageId, offset: 0 };
      requestRef.current = { key, messageId, align: "start" };
      refresh((value) => value + 1);
    },
    [view],
  );
  const navigate = useCallback(
    (where: "top" | "up" | "down" | "bottom") => {
      if (where === "bottom") {
        const node = getScroller();
        // A distant jump crosses unmeasured reclaimed ranges. Land at the
        // destination immediately; reserve animation for nearby content.
        const nearby =
          node &&
          node.scrollHeight - node.scrollTop - node.clientHeight <
            node.clientHeight * 2;
        follow(nearby ? "smooth" : "auto");
        return;
      }
      const node = getScroller();
      const all = latest.current.keys;
      if (!node || !all.length) return;
      if (where === "top") {
        view.followLatest = false;
        if (view.retained.has(all[0]) || all.length <= high) {
          view.anchor = undefined;
          writeScroll(0, "smooth");
        } else reveal(all[0]);
        return;
      }
      const rows = [
        ...node.querySelectorAll<HTMLElement>('[data-rua-chat-role="user"]'),
      ];
      const top = node.getBoundingClientRect().top;
      const ordered = rows.map((row) => ({
        row,
        offset: row.getBoundingClientRect().top - top + node.scrollTop,
      }));
      const destination =
        where === "up"
          ? ordered.reverse().find((item) => item.offset < node.scrollTop - 8)
          : ordered.find((item) => item.offset > node.scrollTop + 8);
      const current =
        virtRef.current.getVirtualItemForOffset(node.scrollTop)?.index ?? 0;
      const nextIndex = Math.max(
        0,
        Math.min(all.length - 1, current + (where === "up" ? -1 : 1)),
      );
      const currentTurn = destination?.row.closest<HTMLElement>(
        "[data-rua-turn-key]",
      )?.dataset.ruaTurnKey;
      // An unloaded turn between two retained islands must not be skipped.
      const candidateIndex = currentTurn
        ? latest.current.keyIndex.get(currentTurn)
        : undefined;
      if (
        candidateIndex !== undefined &&
        (all.length <= high || Math.abs(candidateIndex - current) <= 1)
      ) {
        view.followLatest = false;
        view.anchor = undefined;
        writeScroll(destination!.offset, "smooth");
      } else reveal(all[nextIndex]);
    },
    [follow, getScroller, high, reveal, view, writeScroll],
  );

  const totalSize = virtualizer.getTotalSize();
  const scrollerBeforeCommit = getScroller();
  if (
    ready &&
    scrollerBeforeCommit &&
    !requestRef.current &&
    Math.abs(scrollerBeforeCommit.scrollTop - view.scrollTop) > 0.5
  ) {
    // Browsers may render the virtual range before delivering our scroll
    // listener. Accept that new position before applying an older anchor.
    view.scrollTop = scrollerBeforeCommit.scrollTop;
    view.followLatest =
      scrollerBeforeCommit.scrollHeight -
        scrollerBeforeCommit.scrollTop -
        scrollerBeforeCommit.clientHeight <=
      (view.followLatest ? 80 : 8);
    view.anchor = view.followLatest ? undefined : capture();
  }
  useLayoutEffect(() => {
    const node = getScroller();
    if (!enabled || !node || !keys.length) return;
    if (!restored.current) {
      restored.current = true;
      if (view.followLatest) follow();
      else if (view.anchor && keyIndex.has(view.anchor.turnKey)) {
        view.retained.add(view.anchor.turnKey);
        requestRef.current = {
          key: view.anchor.turnKey,
          messageId: view.anchor.messageId,
          align: "start",
          offset: view.anchor.offset,
        };
        refresh((value) => value + 1);
      } else writeScroll(view.scrollTop);
    }
    if (prependAnchor.current) {
      preserve(prependAnchor.current);
      prependAnchor.current = null;
    }
    const request = requestRef.current;
    if (request) {
      const index = keyIndex.get(request.key);
      if (index !== undefined) {
        if (request.align === "end")
          writeScroll(node.scrollHeight, request.behavior);
        else {
          const row = [
            ...node.querySelectorAll<HTMLElement>("[data-rua-turn-key]"),
          ].find((row) => row.dataset.ruaTurnKey === request.key);
          const message = request.messageId
            ? [
                ...(row?.querySelectorAll<HTMLElement>(
                  "[data-rua-chat-message-id]",
                ) ?? []),
              ].find(
                (item) => item.dataset.ruaChatMessageId === request.messageId,
              )
            : undefined;
          if (row) {
            const target = message ?? row;
            writeScroll(
              node.scrollTop +
                target.getBoundingClientRect().top -
                node.getBoundingClientRect().top -
                (request.offset ?? 0),
            );
          } else virtualizer.scrollToIndex(index, { align: request.align });
        }
      }
      requestRef.current = null;
    } else if (view.followLatest) writeScroll(node.scrollHeight);
    else if (view.anchor) preserve(view.anchor);
    if (view.followLatest) view.anchor = undefined;
    else if (!view.anchor) view.anchor = capture();
    setReady(true);
  }, [
    turns,
    revision,
    virtualizer,
    totalSize,
    enabled,
    keys.length,
    follow,
    capture,
    preserve,
    getScroller,
    keyIndex,
    view,
    writeScroll,
  ]);

  const rangeStart = virtualizer.range?.startIndex;
  const rangeEnd = virtualizer.range?.endIndex;
  useEffect(() => {
    if (
      !enabled ||
      !ready ||
      rangeStart === undefined ||
      rangeEnd === undefined ||
      !keys.length
    )
      return;
    const required = readProtected();
    const start = view.followLatest ? keys.length - 1 : rangeStart;
    const end = view.followLatest ? keys.length - 1 : rangeEnd;
    const existing = new Set(view.retained);
    for (const key of activeKeys) existing.add(key);
    if (keys.length <= high) for (const key of keys) existing.add(key);
    const desired = retainConversationTurns({
      keys,
      previous: existing,
      visibleStart: start,
      visibleEnd: end,
      protectedKeys: required,
      high,
      low,
      buffer,
    });
    // First remove only distant completed whole turns; then add at most one
    // nearby missing whole turn per task. Visible turns are supplied immediately.
    const next = new Set([...existing].filter((key) => desired.has(key)));
    const missing = keys
      .map((key, index) => ({ key, index }))
      .filter(({ key }) => desired.has(key) && !next.has(key))
      .sort(
        (a, b) =>
          Math.abs(a.index - (direction.current < 0 ? start : end)) -
          Math.abs(b.index - (direction.current < 0 ? start : end)),
      );
    if (missing.length) next.add(missing[0].key);
    const changed =
      next.size !== view.retained.size ||
      [...next].some((key) => !view.retained.has(key));
    if (!changed) return;
    const timer = setTimeout(
      () => {
        view.retained = next;
        refresh((value) => value + 1);
      },
      pointerActive.current && next.size >= view.retained.size ? 48 : 0,
    );
    return () => clearTimeout(timer);
  }, [
    enabled,
    ready,
    rangeStart,
    rangeEnd,
    keys,
    high,
    low,
    activeKeys,
    view,
    revision,
    readProtected,
  ]);

  const previousKeys = useRef<ReadonlySet<string>>(new Set());
  useEffect(() => {
    if (!keys.length) return;
    const valid = new Set(keys);
    const removed = new Set(
      [...previousKeys.current].filter((key) => !valid.has(key)),
    );
    previousKeys.current = valid;
    for (const key of removed) {
      view.sizes.delete(key);
      view.retained.delete(key);
    }
    if (removed.size)
      for (const key of view.values.keys()) {
        const scope = decodeURIComponent(key.split("/")[1] ?? "");
        if (scope.startsWith("turn:") && removed.has(scope.slice(5)))
          view.values.delete(key);
      }
  }, [keys, view]);

  useEffect(() => {
    view.onProtectionChange = () => refresh((value) => value + 1);
    return () => {
      view.onProtectionChange = undefined;
    };
  }, [view]);
  useEffect(() => {
    if (!controllerRef) return;
    controllerRef.current = {
      navigate,
      reveal,
      follow,
      beforePrepend: () => {
        prependAnchor.current = capture() ?? null;
        if (prependAnchor.current) view.anchor = prependAnchor.current;
      },
    };
    return () => {
      controllerRef.current = null;
    };
  }, [controllerRef, navigate, reveal, follow, capture, view]);
  useEffect(() => {
    const node = getScroller();
    if (!node) return;
    const pause = () => {
      view.followLatest = false;
      requestRef.current = null;
      programmatic.current = null;
      view.anchor = capture();
      view.scrollTop = node.scrollTop;
    };
    const scroll = () => {
      direction.current = Math.sign(node.scrollTop - lastScroll.current);
      lastScroll.current = node.scrollTop;
      const ownScroll =
        programmatic.current !== null &&
        Math.abs(programmatic.current - node.scrollTop) < 1;
      view.followLatest = followAfterScroll({
        following: view.followLatest,
        direction: direction.current,
        distanceFromBottom:
          node.scrollHeight - node.scrollTop - node.clientHeight,
        programmatic: ownScroll,
      });
      programmatic.current = null;
      view.scrollTop = node.scrollTop;
      // Our own compensation can precede refreshed measurements. Keep the
      // stable anchor until the next user scroll instead of clearing it from
      // a temporarily stale virtual index during width changes.
      if (!view.followLatest && !ownScroll) view.anchor = capture();
      latest.current.onScrollState?.(view.followLatest, true, node.scrollTop);
      if (idleTimer.current) clearTimeout(idleTimer.current);
      idleTimer.current = setTimeout(
        () =>
          latest.current.onScrollState?.(
            view.followLatest,
            false,
            node.scrollTop,
          ),
        3000,
      );
      if (node.scrollTop <= 48) latest.current.onTop?.();
    };
    const wheel = (event: WheelEvent) => {
      if (event.deltaY < 0) pause();
    };
    const touch = () => {
      pointerActive.current = true;
      pause();
    };
    const end = () => {
      pointerActive.current = false;
      refresh((value) => value + 1);
    };
    const hasMessageSelection = () => {
      const selection = window.getSelection();
      return Boolean(
        selection &&
          !selection.isCollapsed &&
          ((selection.anchorNode && node.contains(selection.anchorNode)) ||
            (selection.focusNode && node.contains(selection.focusNode))),
      );
    };
    let hadMessageSelection = hasMessageSelection();
    const refreshProtection = () => {
      if (!view.followLatest) view.anchor = capture();
      refresh((value) => value + 1);
    };
    const selection = () => {
      const hasSelection = hasMessageSelection();
      const relevant = hasSelection || hadMessageSelection;
      hadMessageSelection = hasSelection;
      if (relevant) refreshProtection();
    };
    const playingMedia = new Map<HTMLMediaElement, string>();
    const mediaChange = (event: Event) => {
      const media = event.target;
      if (!(media instanceof HTMLMediaElement)) return;
      const key = media.closest<HTMLElement>("[data-rua-turn-key]")?.dataset
        .ruaTurnKey;
      if (!key) return;
      const wasPlaying = playingMedia.has(media);
      const playing = !media.paused && !media.ended;
      if (playing === wasPlaying) return;
      const count = (view.protectedScopes.get(key) ?? 0) + (playing ? 1 : -1);
      if (count > 0) view.protectedScopes.set(key, count);
      else view.protectedScopes.delete(key);
      if (playing) playingMedia.set(media, key);
      else playingMedia.delete(media);
      refresh((value) => value + 1);
    };
    node.addEventListener("scroll", scroll, { passive: true });
    node.addEventListener("wheel", wheel, { passive: true });
    node.addEventListener("touchmove", touch, { passive: true });
    node.addEventListener("touchend", end, { passive: true });
    node.addEventListener("touchcancel", end, { passive: true });
    document.addEventListener("selectionchange", selection);
    node.addEventListener("focusin", refreshProtection);
    node.addEventListener("focusout", refreshProtection);
    node.addEventListener("play", mediaChange, true);
    node.addEventListener("pause", mediaChange, true);
    node.addEventListener("ended", mediaChange, true);
    const resizeViewport = () => {
      const previousHeight = view.height;
      const nextHeight = node.clientHeight;
      if (
        view.width !== undefined &&
        Math.abs(view.width - node.clientWidth) > 1
      ) {
        view.sizes.clear();
        virtualizer.measure();
      }
      view.width = node.clientWidth;
      if (view.followLatest) writeScroll(node.scrollHeight);
      else if (view.anchor) preserve(view.anchor);
      else if (
        previousHeight !== undefined &&
        Math.abs(previousHeight - nextHeight) > 1
      ) {
        const anchoredScrollTop = Math.max(
          0,
          node.scrollTop + previousHeight - nextHeight,
        );
        writeScroll(anchoredScrollTop);
      }
      view.height = nextHeight;
    };
    const resize =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(resizeViewport);
    resize?.observe(node);
    window.visualViewport?.addEventListener("resize", resizeViewport);
    window.visualViewport?.addEventListener("scroll", resizeViewport);
    return () => {
      node.removeEventListener("scroll", scroll);
      node.removeEventListener("wheel", wheel);
      node.removeEventListener("touchmove", touch);
      node.removeEventListener("touchend", end);
      node.removeEventListener("touchcancel", end);
      document.removeEventListener("selectionchange", selection);
      node.removeEventListener("focusin", refreshProtection);
      node.removeEventListener("focusout", refreshProtection);
      node.removeEventListener("play", mediaChange, true);
      node.removeEventListener("pause", mediaChange, true);
      node.removeEventListener("ended", mediaChange, true);
      for (const key of playingMedia.values()) {
        const count = (view.protectedScopes.get(key) ?? 1) - 1;
        if (count > 0) view.protectedScopes.set(key, count);
        else view.protectedScopes.delete(key);
      }
      resize?.disconnect();
      window.visualViewport?.removeEventListener("resize", resizeViewport);
      window.visualViewport?.removeEventListener("scroll", resizeViewport);
      if (idleTimer.current) clearTimeout(idleTimer.current);
      view.scrollTop = node.scrollTop;
    };
  }, [getScroller, view, capture, virtualizer, writeScroll, preserve]);

  const visible = enabled ? virtualizer.getVirtualItems() : [];
  const mounted =
    !enabled || keys.length <= high
      ? new Set(keys.map((_, index) => index))
      : new Set(
          visible.length
            ? visible.map((item) => item.index)
            : keys.flatMap((key, index) =>
                view.retained.has(key) || activeKeys.has(key) ? [index] : [],
              ),
        );
  return {
    listRef,
    viewState: view,
    virtualizer,
    mounted,
    enabled,
    ready,
    reveal,
  };
}
