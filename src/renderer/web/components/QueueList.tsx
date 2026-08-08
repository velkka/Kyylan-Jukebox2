import { useEffect, useState } from 'react'
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors
} from '@dnd-kit/core'
import {
  SortableContext,
  arrayMove,
  useSortable,
  verticalListSortingStrategy
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import type { QueueEntry } from '@shared/types'
import { useJukebox } from '../JukeboxContext'
import { clearQueue, moveEntry } from '../api'
import { subtitle } from '../util'
import TrackArt from './TrackArt'

export default function QueueList({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const { queue, isAdmin, remove } = useJukebox()
  const serverEntries = queue?.queue ?? []
  // Local copy so drag reordering feels instant; re-syncs from server updates.
  const [entries, setEntries] = useState<QueueEntry[]>(serverEntries)
  useEffect(() => setEntries(serverEntries), [queue])

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }))

  function onDragEnd(event: DragEndEvent): void {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const oldIndex = entries.findIndex((e) => e.id === active.id)
    const newIndex = entries.findIndex((e) => e.id === over.id)
    if (oldIndex < 0 || newIndex < 0) return
    setEntries((prev) => arrayMove(prev, oldIndex, newIndex))
    moveEntry(Number(active.id), newIndex).catch((err) => onError(String(err.message ?? err)))
  }

  const rows = entries.map((e, i) => (
    <Row key={e.id} entry={e} index={i} draggable={isAdmin} isAdmin={isAdmin} onRemove={remove} onError={onError} />
  ))

  return (
    <section className="rounded-2xl bg-jukebox-panel p-4">
      <div className="mb-3 flex items-center justify-between">
        <h3 className="font-semibold">Up next</h3>
        <div className="flex items-center gap-2">
          <span className="text-xs text-white/40">
            {entries.length} queued{isAdmin && entries.length > 1 ? ' · drag to reorder' : ''}
          </span>
          {isAdmin && entries.length > 0 && (
            <button
              onClick={() => clearQueue().catch((err) => onError(String(err.message ?? err)))}
              className="rounded-md bg-white/5 px-2 py-0.5 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
            >
              Clear
            </button>
          )}
        </div>
      </div>

      {entries.length === 0 ? (
        <p className="py-6 text-center text-sm text-white/35">
          Nothing queued yet — add songs from the library below.
        </p>
      ) : isAdmin ? (
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
          <SortableContext items={entries.map((e) => e.id)} strategy={verticalListSortingStrategy}>
            <ul className="space-y-1">{rows}</ul>
          </SortableContext>
        </DndContext>
      ) : (
        <ul className="space-y-1">{rows}</ul>
      )}
    </section>
  )
}

function Row({
  entry,
  index,
  draggable,
  isAdmin,
  onRemove,
  onError
}: {
  entry: QueueEntry
  index: number
  draggable: boolean
  isAdmin: boolean
  onRemove: (id: number) => Promise<void>
  onError: (msg: string) => void
}): JSX.Element {
  const sortable = useSortable({ id: entry.id, disabled: !draggable })
  const style = draggable
    ? { transform: CSS.Transform.toString(sortable.transform), transition: sortable.transition }
    : undefined

  return (
    <li
      ref={draggable ? sortable.setNodeRef : undefined}
      style={style}
      className={`flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-white/5 ${
        sortable.isDragging ? 'bg-white/10 opacity-80' : ''
      }`}
    >
      {draggable && (
        <button
          {...sortable.attributes}
          {...sortable.listeners}
          className="cursor-grab touch-none px-1 text-white/30 hover:text-white/60"
          title="Drag to reorder"
          aria-label="Drag to reorder"
        >
          ⠿
        </button>
      )}
      <span className="w-5 shrink-0 text-center text-xs text-white/30">{index + 1}</span>
      <TrackArt hash={entry.track.artHash} size={40} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{entry.track.title}</p>
        <p className="truncate text-xs text-white/50">
          {subtitle(entry.track.artist, entry.track.album)}
        </p>
      </div>
      {entry.addedByName && (
        <span className="hidden shrink-0 text-xs text-white/35 sm:inline">{entry.addedByName}</span>
      )}
      {(entry.mine || isAdmin) && (
        <button
          onClick={() => onRemove(entry.id).catch((err) => onError(String(err.message ?? err)))}
          className="shrink-0 rounded-md px-2 py-1 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
          title={entry.mine ? 'Remove your song' : 'Remove (admin)'}
          aria-label="Remove song"
        >
          ✕
        </button>
      )}
    </li>
  )
}
