import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { IconSidebarSimple } from "./icons";

/** App frame with an elevated content card between resizable, collapsible
    sidebars (ported from zett-app's Svelte shell). Structural styles live
    in theme.css under "App shell".

    - Drag the seam beside the content card to resize; hold Shift to move
      both sidebars symmetrically.
    - Dragging a sidebar below half its minimum (or squeezing the content
      below its minimum) snaps it closed.
    - On window resize, sidebars shrink to protect the content minimum but
      remember the widths the user chose and grow back when room returns;
      sidebars the shell itself closed reopen themselves.
    - Opening a sidebar when there is no room for it inline shows it as an
      overlay above the content, dismissed by clicking the scrim.

    Numeric layout state lives in a mutable model + CSS custom properties
    (--shell-*-w) written straight to the root element, so dragging never
    re-renders React; only the booleans that change classes/icons go
    through state. */

const CLOSED = 8; // px; a sidebar at this width is "closed"

export interface ShellProps {
  /** The elevated center card. */
  children?: ReactNode;
  titlebar?: ReactNode;
  leftSidebar?: ReactNode;
  rightSidebar?: ReactNode;
  /** Imperative handle, so hosts can reveal a panel they need the user to see
   * (e.g. clicking an error opens the chat with a prompt waiting). */
  shellApiRef?: React.MutableRefObject<ShellApi | null>;
  /** Native macOS overlay traffic lights: spans the titlebar over the left
      sidebar column and reserves room for the lights, so the collapse
      button rides the sidebar's edge. */
  macTrafficLights?: boolean;
  leftMinWidth?: number;
  rightMinWidth?: number;
  minContentWidth?: number;
  defaultLeftWidth?: number;
  defaultRightWidth?: number;
}

interface SideModel {
  enabled: boolean;
  min: number;
  overlayWidth: number;
  width: number;
  /** Last width the user deliberately chose — the "debt" reflow repays. */
  prevUserSet: number;
  /** Overlay mode pins the visual width here while the column stays closed. */
  styleOverride: string | null;
  resizing: boolean;
  /** Closed by the user (stay closed) vs. closed by reflow (reopen when room returns). */
  userClosed: boolean;
  overlay: boolean;
  /** Keep animating through the reflow that a programmatic reopen triggers. */
  overrideAnimate: boolean;
  animateTimer: number | null;
  dropTimer: number | null;
}

interface Model {
  left: SideModel;
  right: SideModel;
  focus: "left" | "right" | "window" | null;
  otherHover: boolean;
  settleTimer: number | null;
  contentMin: number;
}

interface Flags {
  leftClosed: boolean;
  rightClosed: boolean;
  overlayLeft: boolean;
  overlayRight: boolean;
  animateLeft: boolean;
  animateRight: boolean;
  otherHover: boolean;
}

function makeSide(enabled: boolean, min: number, initial: number): SideModel {
  return {
    enabled,
    min: enabled ? min : 0,
    overlayWidth: Math.max(200, min),
    width: enabled ? initial : 0,
    prevUserSet: enabled ? initial : 0,
    styleOverride: null,
    resizing: false,
    userClosed: !enabled,
    overlay: false,
    overrideAnimate: false,
    animateTimer: null,
    dropTimer: null,
  };
}

export interface ShellApi {
  /** Open the right sidebar if it is closed; no-op when already open. */
  openRight(): void;
}

export function Shell({
  children,
  titlebar,
  leftSidebar,
  rightSidebar,
  shellApiRef,
  macTrafficLights = false,
  leftMinWidth = 132,
  rightMinWidth = 132,
  minContentWidth = 264,
  defaultLeftWidth = 300,
  defaultRightWidth = 300,
}: ShellProps) {
  const hasLeft = leftSidebar != null;
  const hasRight = rightSidebar != null;
  const appRef = useRef<HTMLDivElement>(null);

  const modelRef = useRef<Model | null>(null);
  if (modelRef.current === null) {
    modelRef.current = {
      left: makeSide(hasLeft, leftMinWidth, defaultLeftWidth),
      right: makeSide(hasRight, rightMinWidth, defaultRightWidth),
      focus: null,
      otherHover: false,
      settleTimer: null,
      contentMin: minContentWidth,
    };
  }
  const m = modelRef.current;
  m.contentMin = minContentWidth;

  const [flags, setFlags] = useState<Flags>(() => ({
    leftClosed: m.left.width <= CLOSED,
    rightClosed: m.right.width <= CLOSED,
    overlayLeft: false,
    overlayRight: false,
    animateLeft: true,
    animateRight: true,
    otherHover: false,
  }));

  /** Push the model out: widths as CSS vars (imperative, no re-render),
      class-affecting booleans through state (bails out when unchanged). */
  const commit = () => {
    const el = appRef.current;
    if (el) {
      el.style.setProperty("--shell-left-w", `${m.left.width}px`);
      el.style.setProperty("--shell-left-style-w", m.left.styleOverride ?? `${m.left.width}px`);
      el.style.setProperty("--shell-right-w", `${m.right.width}px`);
      el.style.setProperty("--shell-right-style-w", m.right.styleOverride ?? `${m.right.width}px`);
    }
    setFlags((prev) => {
      const next: Flags = {
        leftClosed: m.left.width <= CLOSED,
        rightClosed: m.right.width <= CLOSED,
        overlayLeft: m.left.overlay,
        overlayRight: m.right.overlay,
        animateLeft: !m.left.resizing || m.left.overrideAnimate,
        animateRight: !m.right.resizing || m.right.overrideAnimate,
        otherHover: m.otherHover,
      };
      for (const k of Object.keys(next) as (keyof Flags)[]) {
        if (prev[k] !== next[k]) return next;
      }
      return prev;
    });
  };

  /** Resize one sidebar toward `size`, never letting the content drop
      below its minimum. */
  const setSideWidth = (side: SideModel, size: number) => {
    const el = appRef.current;
    if (!el || !side.enabled) return;
    const contentWidth = el.getBoundingClientRect().width - m.left.width - m.right.width;
    const change = Math.max(
      side.width - Math.max(size, CLOSED),
      m.contentMin - contentWidth,
    );
    side.width -= change;
  };

  /** Snap widths to legal values after a drag or reflow. A sidebar dragged
      below half its minimum — or one whose minimum no longer fits — closes;
      anything else lands on its minimum. */
  const settle = (byUser: boolean) => {
    const el = appRef.current;
    if (!el) return;
    const capacity = el.getBoundingClientRect().width;

    m.left.resizing = false;
    m.right.resizing = false;
    m.focus = null;

    const openLeft = m.left.width !== CLOSED ? Math.max(m.left.width, m.left.min) : CLOSED;
    const openRight = m.right.width !== CLOSED ? Math.max(m.right.width, m.right.min) : CLOSED;
    const tight = capacity - (openLeft + openRight) <= m.contentMin;
    const closeFit =
      (m.left.width < m.left.min && tight) || (m.right.width < m.right.min && tight);

    for (const side of [m.left, m.right]) {
      if (side.width < side.min) {
        const close = side.width < side.min / 2 || closeFit;
        if (!close) side.prevUserSet = side.min;
        if (close && side.width > CLOSED) side.userClosed = byUser;
        side.width = close ? CLOSED : side.min;
      } else if (byUser && side.enabled) {
        side.prevUserSet = side.width;
      }
    }
    commit();
  };

  /** Reflow after the shell resizes: shrink sidebars (larger side first) to
      protect the content minimum, then spend any regained room reopening
      sidebars the shell closed and repaying remembered widths. */
  const reflow = (newSize: number, settleDelay: number) => {
    const el = appRef.current;
    if (!el) return;

    m.focus = "window";
    m.left.resizing = true;
    m.right.resizing = true;

    let needed = m.contentMin - (newSize - (m.left.width + m.right.width));
    if (needed > 0) {
      const difference = m.left.width - m.right.width;
      if (difference !== 0) {
        const take = Math.min(Math.abs(difference), needed);
        if (difference < 0) setSideWidth(m.right, m.right.width - take);
        else setSideWidth(m.left, m.left.width - take);
        needed = m.contentMin - (newSize - (m.left.width + m.right.width));
      }
      if (needed !== 0) {
        // one side wasn't (enough of) a donor; take from both evenly
        const share = m.left.enabled && m.right.enabled ? needed / 2 : needed;
        if (m.right.enabled) m.right.width = Math.max(m.right.width - share, CLOSED);
        if (m.left.enabled) m.left.width = Math.max(m.left.width - share, CLOSED);
        // anything still owed means the window is narrower than the shell minimum
      }
    }

    const debt =
      m.left.prevUserSet - m.left.width + (m.right.prevUserSet - m.right.width);
    if (debt !== 0) {
      let capacity =
        el.getBoundingClientRect().width - m.left.width - m.right.width - m.contentMin;

      const needsReopen = (s: SideModel) => s.width <= CLOSED && !s.userClosed;
      const reopenCost = (s: SideModel) => (needsReopen(s) ? s.min - CLOSED : 0);

      if (capacity > reopenCost(m.left) + reopenCost(m.right)) {
        capacity -= reopenCost(m.left) + reopenCost(m.right);
        for (const side of [m.left, m.right]) {
          if (!needsReopen(side)) continue;
          side.width = side.min;
          side.overrideAnimate = true;
          if (side.animateTimer !== null) clearTimeout(side.animateTimer);
          side.animateTimer = window.setTimeout(() => {
            side.overrideAnimate = false;
            side.animateTimer = null;
            commit();
          }, 500);
        }
      }

      // nothing left to reopen → grow toward remembered widths, evening
      // the two debts out before splitting what remains. Closed sidebars
      // accrue no debt: a user-closed sidebar still remembers prevUserSet
      // for its next manual open, but growing it here would pop it open
      // (or inflate a shell-closed one into a sliver) on window resize.
      if (!needsReopen(m.left) && !needsReopen(m.right)) {
        let leftWant = m.left.width <= CLOSED ? 0 : m.left.prevUserSet - m.left.width;
        let rightWant = m.right.width <= CLOSED ? 0 : m.right.prevUserSet - m.right.width;

        if (leftWant > rightWant) {
          const d = Math.min(leftWant - rightWant, capacity);
          leftWant -= d;
          capacity -= d;
          m.left.width += d;
        }
        if (rightWant > leftWant) {
          const d = Math.min(rightWant - leftWant, capacity);
          rightWant -= d;
          capacity -= d;
          m.right.width += d;
        }

        const each = Math.min(leftWant, capacity) / 2;
        if (m.left.enabled) m.left.width += each;
        if (m.right.enabled) m.right.width += each;
      }
    }

    for (const side of [m.left, m.right]) {
      if (side.width >= side.overlayWidth && side.overlay) {
        side.styleOverride = null;
        side.overlay = false;
      }
      if (
        side.overlay &&
        side.prevUserSet < side.overlayWidth &&
        side.prevUserSet === side.width
      ) {
        dropOverlay(side);
      }
    }

    if (m.settleTimer !== null) clearTimeout(m.settleTimer);
    if (settleDelay === 0) {
      settle(false);
    } else {
      m.settleTimer = window.setTimeout(() => settle(false), settleDelay);
    }
    commit();
  };

  const dropOverlay = (side: SideModel) => {
    side.styleOverride = null;
    if (side.dropTimer !== null) clearTimeout(side.dropTimer);
    side.dropTimer = window.setTimeout(() => {
      side.overlay = false;
      side.dropTimer = null;
      commit();
    }, 250);
    commit();
  };

  // Imperative handle: reveal the right panel without toggling it shut when
  // it is already open (toggleSide alone would hide it).
  useEffect(() => {
    if (!shellApiRef) return;
    shellApiRef.current = {
      openRight() {
        if (!hasRight) return;
        if (m.right.width >= m.right.min) return;
        toggleSide(m.right, m.left);
      },
    };
    return () => {
      shellApiRef.current = null;
    };
  });

  /** Titlebar button: close an open sidebar, or reopen it — inline when it
      fits, as an overlay above the content when it doesn't. */
  const toggleSide = (side: SideModel, other: SideModel) => {
    const el = appRef.current;
    if (!el || m.left.overlay || m.right.overlay) return;

    if (side.width >= side.min) {
      side.width = CLOSED;
      side.userClosed = true;
      commit();
      return;
    }

    side.userClosed = false;
    const capacity = el.getBoundingClientRect().width - other.width - m.contentMin;
    if (capacity >= side.min) {
      side.width = Math.min(capacity, side.prevUserSet);
    } else {
      side.styleOverride = `${side.overlayWidth}px`;
      side.overlay = true;
    }
    commit();
  };

  const beginDrag = (e: ReactPointerEvent, isLeft: boolean) => {
    e.preventDefault();
    const side = isLeft ? m.left : m.right;
    const other = isLeft ? m.right : m.left;
    side.resizing = true;
    if (m.otherHover && other.enabled) other.resizing = true;
    m.focus = isLeft ? "left" : "right";
    commit();
  };

  // Sync sidebar presence when the props appear/disappear (e.g. a panel
  // that only exists once a document is open), then reflow so the content
  // minimum still holds.
  useLayoutEffect(() => {
    let changed = false;
    const sync = (side: SideModel, enabled: boolean, min: number, initial: number) => {
      side.min = enabled ? min : 0;
      side.overlayWidth = Math.max(200, min);
      if (side.enabled === enabled) return;
      side.enabled = enabled;
      changed = true;
      if (enabled) {
        side.width = initial;
        side.prevUserSet = initial;
        side.userClosed = false;
      } else {
        side.width = 0;
        side.prevUserSet = 0;
        side.userClosed = true;
        side.overlay = false;
        side.styleOverride = null;
      }
    };
    sync(m.left, hasLeft, leftMinWidth, defaultLeftWidth);
    sync(m.right, hasRight, rightMinWidth, defaultRightWidth);
    const el = appRef.current;
    if (changed && el) reflow(el.getBoundingClientRect().width, 0);
    else commit();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasLeft, hasRight, leftMinWidth, rightMinWidth, defaultLeftWidth, defaultRightWidth]);

  useLayoutEffect(() => {
    const el = appRef.current;
    if (!el) return;

    let first = true;
    const ro = new ResizeObserver(() => {
      reflow(el.getBoundingClientRect().width, first ? 0 : 500);
      first = false;
    });
    ro.observe(el);

    const onPointerMove = (e: PointerEvent) => {
      if (m.focus !== "left" && m.focus !== "right") return;
      const rect = el.getBoundingClientRect();
      if (m.focus === "left") {
        setSideWidth(m.left, e.clientX - rect.left);
        if (m.otherHover) setSideWidth(m.right, Math.max(e.clientX - rect.left, m.right.min));
      } else {
        setSideWidth(m.right, rect.right - e.clientX);
        if (m.otherHover) setSideWidth(m.left, Math.max(rect.right - e.clientX, m.left.min));
      }
      commit();
    };
    const onPointerUp = () => {
      if (m.focus === null && !m.left.resizing && !m.right.resizing) return;
      settle(true);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Shift") return;
      m.otherHover = true;
      if (m.left.resizing && m.right.enabled) m.right.resizing = true;
      else if (m.right.resizing && m.left.enabled) m.left.resizing = true;
      commit();
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key !== "Shift") return;
      m.otherHover = false;
      commit();
    };

    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerUp);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      ro.disconnect();
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerUp);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      for (const side of [m.left, m.right]) {
        if (side.animateTimer !== null) clearTimeout(side.animateTimer);
        if (side.dropTimer !== null) clearTimeout(side.dropTimer);
      }
      if (m.settleTimer !== null) clearTimeout(m.settleTimer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mac = macTrafficLights;
  const collapser = (side: "left" | "right") => {
    const closed = side === "left" ? flags.leftClosed : flags.rightClosed;
    return (
      <button
        type="button"
        className="shell-collapser"
        aria-label={`${closed ? "Open" : "Close"} ${side} sidebar`}
        onClick={() =>
          side === "left" ? toggleSide(m.left, m.right) : toggleSide(m.right, m.left)
        }
      >
        {/* Flip so the icon's panel sits on the side the button controls.
            Phosphor's `mirrored` prop won't do: it emits a transform
            *attribute* on the svg root, which WebKit ignores. */}
        <IconSidebarSimple
          size={16}
          style={side === "right" ? { transform: "scale(-1, 1)" } : undefined}
        />
      </button>
    );
  };

  return (
    <div
      ref={appRef}
      className={"shell" + (flags.otherHover ? " shell-other-hover" : "")}
      style={
        {
          "--shell-left-min": `${hasLeft ? leftMinWidth : 0}px`,
          "--shell-right-min": `${hasRight ? rightMinWidth : 0}px`,
          "--shell-content-min": `${minContentWidth}px`,
        } as CSSProperties
      }
    >
      <div
        role="none"
        className={
          "absolute left-0 top-0 z-[90] h-full w-full bg-black transition-opacity duration-200 " +
          (flags.overlayLeft || flags.overlayRight
            ? "opacity-25"
            : "pointer-events-none opacity-0")
        }
        onClick={() => {
          if (m.left.overlay) dropOverlay(m.left);
          if (m.right.overlay) dropOverlay(m.right);
        }}
      />

      <div
        data-tauri-drag-region="deep"
        className={
          "shell-titlebar row-start-1 " + (mac ? "col-span-2 col-start-1" : "col-start-2")
        }
      >
        {mac && <div className={"shell-lights" + (flags.animateLeft ? " shell-animate" : "")} />}
        {hasLeft && collapser("left")}
        <div className="shell-titlebar-user">{titlebar}</div>
        {hasRight && collapser("right")}
      </div>

      <div
        className={
          "shell-left-wrap col-start-1 h-full w-full " +
          (mac ? "row-start-2" : "row-span-2 row-start-1") +
          (flags.animateLeft ? " shell-animate" : "")
        }
      >
        <div className={"shell-left h-full w-full" + (flags.overlayLeft ? " shell-overlay" : "")}>
          {leftSidebar}
        </div>
      </div>

      <div className="shell-main col-start-2 row-start-2 h-full w-full">
        <div className="shell-content-wrap h-full w-full">
          {hasLeft && (
            <div
              role="none"
              className="shell-handle shell-handle-left"
              onPointerDown={(e) => beginDrag(e, true)}
            >
              <div className="shell-handle-view" />
            </div>
          )}
          <div className="shell-content">{children}</div>
          {hasRight && (
            <div
              role="none"
              className="shell-handle shell-handle-right"
              onPointerDown={(e) => beginDrag(e, false)}
            >
              <div className="shell-handle-view" />
            </div>
          )}
        </div>
      </div>

      <div
        className={
          "shell-right-wrap col-start-3 row-span-2 row-start-1 h-full w-full" +
          (flags.overlayRight ? " shell-overlay" : "") +
          (flags.animateRight ? " shell-animate" : "")
        }
      >
        <div
          className={"shell-right h-full w-full" + (flags.overlayRight ? " shell-overlay" : "")}
        >
          <div className="shell-right-inner relative h-full w-full">{rightSidebar}</div>
        </div>
      </div>
    </div>
  );
}
