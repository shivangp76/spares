import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useLocation } from "react-router-dom";

/** A small downward arrow after a menu button's label. */
function Caret() {
  return (
    <svg
      width="0.7em"
      height="0.7em"
      viewBox="0 0 10 10"
      aria-hidden
      style={{ marginLeft: "0.35em", verticalAlign: "0.05em" }}
    >
      <path
        d="M1 3l4 4 4-4"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.5}
      />
    </svg>
  );
}

/**
 * A navbar button that opens a menu of `children` (links or buttons with `nav-menu-item`) below
 * it. The menu is placed with `position: fixed` in `<body>`, so the phone navbar's sideways
 * scrolling row doesn't clip it. It closes on picking an item, clicking elsewhere, Escape or going to another
 * page.
 */
export default function NavMenu({
  label,
  title,
  className = "navbar-button",
  current = false,
  align = "left",
  children,
}: {
  label: React.ReactNode;
  title?: string;
  className?: string;
  /** Whether the page shown is one of the menu's, to mark the button as the current link */
  current?: boolean;
  /** Which edge of the button the menu lines up with */
  align?: "left" | "right";
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<React.CSSProperties>({});
  const button = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const id = useId();
  const { pathname } = useLocation();

  // eslint-disable-next-line react-hooks/set-state-in-effect -- going to another page closes it
  useEffect(() => setOpen(false), [pathname]);

  useLayoutEffect(() => {
    if (!open || !button.current) return;
    const rect = button.current.getBoundingClientRect();
    const width = menu.current?.offsetWidth ?? 0;
    const wanted = align === "right" ? rect.right - width : rect.left;
    // Kept on screen, as a button scrolled partly out of the phone navbar's row would put it off
    const left = Math.max(
      8,
      Math.min(wanted, document.documentElement.clientWidth - width - 8),
    );
    setPosition({ top: rect.bottom + 4, left });
  }, [open, align]);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(e: PointerEvent) {
      const target = e.target as Node;
      if (!menu.current?.contains(target) && !button.current?.contains(target))
        setOpen(false);
    }
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
      button.current?.focus();
    }
    // The menu stays where the button was, so scrolling or resizing closes it
    const close = () => setOpen(false);
    document.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown, { capture: true });
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, { capture: true, passive: true });
    menu.current
      ?.querySelector<HTMLElement>(".nav-menu-item:not(:disabled)")
      ?.focus();
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown, { capture: true });
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, { capture: true });
    };
  }, [open]);

  /** Arrow keys move between the items, as in a menu. */
  function onMenuKeyDown(e: React.KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [
      ...(menu.current?.querySelectorAll<HTMLElement>(
        ".nav-menu-item:not(:disabled)",
      ) ?? []),
    ];
    const at = items.indexOf(document.activeElement as HTMLElement);
    const next =
      e.key === "ArrowDown"
        ? (at + 1) % items.length
        : (at - 1 + items.length) % items.length;
    items[next]?.focus();
  }

  return (
    <>
      <button
        ref={button}
        type="button"
        className={`${className}${current ? " navbar-link-current" : ""}`}
        title={title}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen((o) => !o)}
      >
        {label}
        <Caret />
      </button>
      {/* In <body>, not the navbar's row, which focusing an item would scroll (closing the menu) */}
      {open &&
        createPortal(
          <div
            ref={menu}
            id={id}
            role="menu"
            className="nav-menu"
            style={position}
            onKeyDown={onMenuKeyDown}
            // Picking an item closes the menu; its own click handler runs first
            onClick={(e) => {
              if ((e.target as HTMLElement).closest(".nav-menu-item"))
                setOpen(false);
            }}
          >
            {children}
          </div>,
          document.body,
        )}
    </>
  );
}
