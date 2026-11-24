'use client';

import { LockKeyhole } from 'lucide-react';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { RoseIcon } from './rose-icon';

/** Compact, accessible content indicators shared by desktop and web list rows. */
export function RegistryContentIndicator({
  kind,
  label,
  description,
}: {
  kind: 'private' | 'nsfw';
  label: string;
  description: string;
}) {
  const Icon = kind === 'private' ? LockKeyhole : RoseIcon;
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const tooltip = useRef<HTMLSpanElement>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0, above: false });

  useLayoutEffect(() => {
    if (!open || !trigger.current || !tooltip.current) return;
    const place = () => {
      if (!trigger.current || !tooltip.current) return;
      const anchor = trigger.current.getBoundingClientRect();
      const { width, height } = tooltip.current.getBoundingClientRect();
      const above = anchor.bottom + height > window.innerHeight - 16;
      setPosition({
        top: Math.max(16, above ? anchor.top - height : anchor.bottom),
        left: Math.max(16, Math.min(anchor.left, window.innerWidth - width - 16)),
        above,
      });
    };
    place();
    // Language and UI-scale changes can resize either end of the tooltip.
    const observer = new ResizeObserver(place);
    observer.observe(trigger.current);
    observer.observe(tooltip.current);
    const scroll = () => {
      const anchor = trigger.current?.getBoundingClientRect();
      // Keyboard focus can scroll the badge into view. Keep its explanation
      // anchored while visible, rather than immediately dismissing it.
      if (
        document.activeElement === trigger.current &&
        anchor &&
        anchor.bottom > 0 &&
        anchor.top < window.innerHeight &&
        anchor.right > 0 &&
        anchor.left < window.innerWidth
      ) {
        place();
      } else {
        setOpen(false);
      }
    };
    document.addEventListener('scroll', scroll, true);
    return () => {
      observer.disconnect();
      document.removeEventListener('scroll', scroll, true);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const dismiss = () => setOpen(false);
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') dismiss();
    };
    const pointerdown = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !trigger.current?.contains(event.target) &&
        !tooltip.current?.contains(event.target)
      )
        dismiss();
    };
    document.addEventListener('keydown', keydown);
    document.addEventListener('pointerdown', pointerdown);
    window.addEventListener('resize', dismiss);
    return () => {
      document.removeEventListener('keydown', keydown);
      document.removeEventListener('pointerdown', pointerdown);
      window.removeEventListener('resize', dismiss);
    };
  }, [open]);

  function leave(target: EventTarget | null) {
    if (document.activeElement === trigger.current) return;
    if (
      target instanceof Node &&
      (trigger.current?.contains(target) || tooltip.current?.contains(target))
    )
      return;
    setOpen(false);
  }

  return (
    <>
      <button
        ref={trigger}
        type="button"
        className={`registry-content-indicator registry-content-indicator--${kind}`}
        aria-label={label}
        aria-describedby={open ? id : undefined}
        onPointerEnter={(event) => {
          if (event.pointerType !== 'touch') setOpen(true);
        }}
        onPointerLeave={(event) => leave(event.relatedTarget)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={(event) => {
          event.stopPropagation();
          setOpen(true);
        }}
      >
        <Icon aria-hidden />
      </button>
      {open
        ? createPortal(
            <span
              ref={tooltip}
              id={id}
              role="tooltip"
              className={`registry-content-tooltip registry-content-tooltip--${kind}`}
              data-placement={position.above ? 'above' : 'below'}
              style={{ top: position.top, left: position.left }}
              onPointerLeave={(event) => leave(event.relatedTarget)}
            >
              <span>
                <strong>{label}</strong>
                <span>{description}</span>
              </span>
            </span>,
            document.body,
          )
        : null}
    </>
  );
}
