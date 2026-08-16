import { useJukebox, useLivePosition } from '../JukeboxContext'
import { playerPause, playerPlay, skipCurrent } from '../api'
import { formatTime, subtitle } from '../util'
import TrackArt from './TrackArt'

export default function NowPlaying(): JSX.Element {
  const { queue, progress, isAdmin, refresh, downvote } = useJukebox()
  const livePosition = useLivePosition(progress)
  const entry = queue?.nowPlaying.entry ?? null
  const np = queue?.nowPlaying
  // 0 = off; negative = enabled but hide the count/threshold.
  const downvotesOn = (np?.downvoteThreshold ?? 0) !== 0
  const showDownvoteCount = (np?.downvoteThreshold ?? 0) > 0
  // When nothing is playing, show an empty bar rather than stale progress.
  const position = entry ? livePosition : 0
  const duration = entry ? progress.duration || entry.track.duration || 0 : 0
  const pct = entry && duration > 0 ? Math.min((position / duration) * 100, 100) : 0

  return (
    <section className="rounded-2xl bg-gradient-to-br from-jukebox-panel to-black/40 p-5 shadow-xl">
      <div className="flex items-center gap-4">
        <TrackArt hash={entry?.track.artHash ?? null} size={72} />
        <div className="min-w-0 flex-1">
          <p className="flex items-center gap-2 text-xs uppercase tracking-wide text-jukebox-accent2">
            {progress.playing ? 'Now playing' : entry ? 'Paused' : 'Nothing playing'}
            {queue?.nowPlaying.isStandby && (
              <span className="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-medium text-white/60">
                STANDBY
              </span>
            )}
          </p>
          <h2 className="truncate text-lg font-semibold">
            {entry ? entry.track.title : 'Queue is empty'}
          </h2>
          <p className="truncate text-sm text-white/55">
            {entry ? subtitle(entry.track.artist, entry.track.album) : 'Add a song to get started'}
          </p>
        </div>
        {entry && (
          <div className="flex shrink-0 items-center gap-2">
            {downvotesOn && np && (
              <button
                onClick={() => downvote()}
                disabled={np.downvotedByMe}
                className="rounded-lg bg-white/10 px-3 py-2 text-sm font-medium tabular-nums hover:bg-red-500/20 disabled:opacity-60 disabled:hover:bg-white/10"
                title={np.downvotedByMe ? 'You downvoted this song' : 'Downvote — vote to skip'}
              >
                👎{showDownvoteCount ? ` ${np.downvotes}/${np.downvoteThreshold}` : ''}
              </button>
            )}
            {isAdmin && (
              <>
                <button
                  onClick={() => (progress.playing ? playerPause() : playerPlay())}
                  className="rounded-lg bg-white/10 px-3 py-2 text-sm font-medium hover:bg-white/20"
                  title={progress.playing ? 'Pause' : 'Play'}
                  aria-label={progress.playing ? 'Pause' : 'Play'}
                >
                  {progress.playing ? '⏸' : '▶'}
                </button>
                <button
                  onClick={() => skipCurrent().then(refresh)}
                  className="rounded-lg bg-white/10 px-3 py-2 text-sm font-medium hover:bg-white/20"
                  title="Skip"
                >
                  ⏭ Skip
                </button>
              </>
            )}
          </div>
        )}
      </div>

      <div className="mt-4">
        <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
          <div
            className="h-full rounded-full bg-jukebox-accent transition-[width] duration-300"
            style={{ width: `${pct}%` }}
          />
        </div>
        <div className="mt-1 flex justify-between text-xs tabular-nums text-white/40">
          <span>{formatTime(position)}</span>
          <span>{formatTime(duration)}</span>
        </div>
      </div>

      {entry?.addedByName && (
        <p className="mt-2 text-xs text-white/40">added by {entry.addedByName}</p>
      )}
    </section>
  )
}
