import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

/** Supplemental help, available by hover, keyboard focus, or tap. */
export function InfoTip({ label, children }: { label: string; children: ReactNode }) {
  const id = useId()
  const trigger = useRef<HTMLButtonElement>(null)
  const tip = useRef<HTMLDivElement>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const [open, setOpen] = useState(false)
  const [position, setPosition] = useState({ left: 12, top: 12 })
  function show() { clearTimeout(timer.current); setOpen(true) }
  function leave() {
    timer.current = setTimeout(() => {
      if (document.activeElement !== trigger.current) setOpen(false)
    }, 180)
  }
  useEffect(() => () => clearTimeout(timer.current), [])
  useLayoutEffect(() => {
    if (!open) return
    function place() {
      if (!trigger.current || !tip.current) return
      const anchor = trigger.current.getBoundingClientRect()
      const box = tip.current.getBoundingClientRect()
      const left = Math.max(12, Math.min(anchor.left, window.innerWidth - box.width - 12))
      const below = anchor.bottom + 8
      const top = below + box.height <= window.innerHeight - 12 ? below : Math.max(12, anchor.top - box.height - 8)
      setPosition({ left, top })
    }
    function dismiss(event: PointerEvent) {
      if (!trigger.current?.contains(event.target as Node) && !tip.current?.contains(event.target as Node)) setOpen(false)
    }
    function escape(event: KeyboardEvent) { if (event.key === 'Escape') setOpen(false) }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('pointerdown', dismiss)
    document.addEventListener('keydown', escape)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('pointerdown', dismiss)
      document.removeEventListener('keydown', escape)
    }
  }, [open])
  return <>
    <button ref={trigger} type="button" className="ob-info-tip-trigger" aria-label={label} aria-describedby={open ? id : undefined} onPointerEnter={event => { if (event.pointerType !== 'touch') show() }} onPointerLeave={leave} onFocus={show} onBlur={() => setOpen(false)} onClick={show}>
      <svg viewBox="0 0 20 20" width="16" height="16" fill="none" stroke="currentColor" aria-hidden="true"><circle cx="10" cy="10" r="7.25" strokeWidth="1.4"/><path d="M10 9v5" strokeWidth="1.5"/><circle cx="10" cy="6.2" r=".9" fill="currentColor" stroke="none"/></svg>
    </button>
    {open && createPortal(<div ref={tip} id={id} role="tooltip" className="ob-info-tip" style={position} onPointerEnter={show} onPointerLeave={leave}>{children}</div>, document.body)}
  </>
}
