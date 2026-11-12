import { useEffect, useRef, useState } from 'react'

export function CopyButton({ value, label, className = 'ob-text-link' }: { value: string; label: string; className?: string }) {
  const [message, setMessage] = useState('')
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const current = useRef(value)
  current.current = value
  useEffect(() => {
    setMessage('')
    clearTimeout(timer.current)
    return () => clearTimeout(timer.current)
  }, [value])
  async function copy() {
    clearTimeout(timer.current)
    try {
      await navigator.clipboard.writeText(value)
      if (current.current !== value) return
      setMessage('Copied')
      timer.current = setTimeout(() => setMessage(''), 2000)
    } catch {
      if (current.current === value) setMessage('Select the text to copy')
    }
  }
  return <button type="button" className={`ob-copy-button ${className}`} disabled={!value} aria-live="polite" onClick={event => { event.stopPropagation(); void copy() }}>
    <svg width="15" height="15" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><rect x="6.5" y="6.5" width="10" height="11" rx="1.5"/><path d="M13 6.5v-3A1.5 1.5 0 0 0 11.5 2h-8A1.5 1.5 0 0 0 2 3.5v9A1.5 1.5 0 0 0 3.5 14h3"/></svg>
    <span>{message || label}</span>
  </button>
}

/** Compact address display for records and dialogs, with the full value copied. */
export function CopyAddress({ value, label = 'Copy address' }: { value: string; label?: string }) {
  if (!value) return <span className="ob-meta">Not available</span>
  return <div className="ob-copy-address"><code>{value}</code><CopyButton value={value} label={label}/></div>
}
