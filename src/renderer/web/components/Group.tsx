/** Titled card used to divide the admin panel into sections. */
export default function Group({
  title,
  action,
  children
}: {
  title: string
  /** Optional controls rendered on the title row, right-aligned. */
  action?: React.ReactNode
  children: React.ReactNode
}): JSX.Element {
  return (
    <div className="rounded-xl bg-black/25 p-3">
      <div className="mb-2 flex items-center gap-2">
        <p className="text-xs font-medium uppercase tracking-wide text-white/40">{title}</p>
        {action && <div className="ml-auto flex items-center gap-1">{action}</div>}
      </div>
      {children}
    </div>
  )
}
