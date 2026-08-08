import { artUrl } from '../api'

export default function TrackArt({
  hash,
  size = 48,
  className = ''
}: {
  hash: string | null
  size?: number
  className?: string
}): JSX.Element {
  const style = { width: size, height: size }
  if (hash) {
    return (
      <img
        src={artUrl(hash)}
        alt=""
        style={style}
        className={`shrink-0 rounded-md object-cover ${className}`}
      />
    )
  }
  return (
    <div
      style={style}
      className={`shrink-0 rounded-md bg-white/5 grid place-items-center text-white/25 ${className}`}
    >
      <span style={{ fontSize: size * 0.5 }}>♪</span>
    </div>
  )
}
