import { useCallback, useEffect, useState } from "react";
import type {
  PlayHistoryItem,
  StatsResponse,
  TrackStat,
  UserStat,
} from "@shared/types";
import * as api from "../api";
import { useJukebox } from "../JukeboxContext";
import Group from "./Group";
import TrackArt from "./TrackArt";

type Tab = "history" | "played" | "downvoted" | "guests";

const TABS: { key: Tab; label: string }[] = [
  { key: "history", label: "Played" },
  { key: "played", label: "Top songs" },
  { key: "downvoted", label: "Downvoted" },
  { key: "guests", label: "Guests" },
];

/** "just now" / "12 min ago" / "3 h ago", falling back to a local date+time. */
function timeAgo(iso: string): string {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return "";
  const mins = Math.floor((Date.now() - then) / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  if (mins < 24 * 60) return `${Math.floor(mins / 60)} h ago`;
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export default function StatsPanel({
  onError,
}: {
  onError: (msg: string) => void;
}): JSX.Element {
  const { isAdmin } = useJukebox();
  const [tab, setTab] = useState<Tab>("history");
  const [stats, setStats] = useState<StatsResponse | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);

  const fail = useCallback(
    (e: unknown) => onError(e instanceof Error ? e.message : String(e)),
    [onError],
  );
  const load = useCallback(() => {
    api.getStats(200).then(setStats).catch(fail);
  }, [fail]);
  useEffect(load, [load]);

  function reset(): void {
    if (!confirmReset) {
      setConfirmReset(true);
      setTimeout(() => setConfirmReset(false), 4000);
      return;
    }
    setConfirmReset(false);
    api.resetStats().then(setStats).catch(fail);
  }

  const t = stats?.totals;
  return (
    <Group
      title="History & stats"
      action={
        <>
          <button
            onClick={load}
            className="rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-white/10 hover:text-white"
            title="Refresh"
          >
            ↻
          </button>
          {isAdmin && (
            <button
              onClick={reset}
              className={`rounded-md px-2 py-0.5 text-xs ${
                confirmReset
                  ? "bg-red-500/25 text-red-200"
                  : "text-white/50 hover:bg-red-500/20 hover:text-red-300"
              }`}
              title="Clear all history and stats"
            >
              {confirmReset ? "Clear everything?" : "Clear"}
            </button>
          )}
        </>
      }
    >
      <div className="-mt-1 mb-2 flex flex-wrap gap-x-3 text-xs text-white/40">
        <span>{t?.plays ?? 0} played</span>
        <span>{t?.requests ?? 0} requests</span>
        <span>{t?.downvotes ?? 0} downvotes</span>
      </div>

      <div className="mb-3 flex gap-1">
        {TABS.map((x) => (
          <button
            key={x.key}
            onClick={() => setTab(x.key)}
            className={`min-w-0 flex-1 truncate rounded-lg px-2 py-1 text-xs transition ${
              tab === x.key
                ? "bg-jukebox-accent/25 text-jukebox-accent"
                : "text-white/50 hover:bg-white/10"
            }`}
          >
            {x.label}
          </button>
        ))}
      </div>

      {tab === "history" && <History items={stats?.history ?? []} />}
      {tab === "played" && (
        <Ranking
          items={stats?.topPlayed ?? []}
          unit="play"
          empty="Nothing played yet."
        />
      )}
      {tab === "downvoted" && (
        <Ranking
          items={stats?.topDownvoted ?? []}
          unit="downvote"
          empty="No downvotes yet."
        />
      )}
      {tab === "guests" && (
        <Guests
          items={stats?.users ?? []}
          onChange={load}
          onError={onError}
          isAdmin={isAdmin}
        />
      )}
    </Group>
  );
}

function Empty({ text }: { text: string }): JSX.Element {
  return <p className="py-2 text-sm text-white/40">{text}</p>;
}

function History({ items }: { items: PlayHistoryItem[] }): JSX.Element {
  if (items.length === 0) return <Empty text="Nothing has played yet." />;
  return (
    <ul className="max-h-80 space-y-1 overflow-y-auto pr-1">
      {items.map((h) => (
        <li key={h.id} className="flex items-center gap-2 text-sm">
          <TrackArt hash={h.artHash} size={32} />
          <div className="min-w-0 flex-1">
            <p className="truncate">{h.title}</p>
            <p className="truncate text-xs text-white/45">
              {h.artist ?? "Unknown"}
              {" · "}
              {h.isStandby ? (
                <span className="text-white/35">standby</span>
              ) : (
                (h.requestedByName ?? "unknown guest")
              )}
            </p>
          </div>
          <span className="shrink-0 text-xs tabular-nums text-white/35">
            {timeAgo(h.playedAt)}
          </span>
        </li>
      ))}
    </ul>
  );
}

function Ranking({
  items,
  unit,
  empty,
}: {
  items: TrackStat[];
  unit: string;
  empty: string;
}): JSX.Element {
  if (items.length === 0) return <Empty text={empty} />;
  return (
    <ol className="max-h-80 space-y-1 overflow-y-auto pr-1">
      {items.map((s, i) => (
        <li key={s.trackId} className="flex items-center gap-2 text-sm">
          <span className="w-5 shrink-0 text-right text-xs tabular-nums text-white/30">
            {i + 1}
          </span>
          <div className="min-w-0 flex-1">
            <p className="truncate">{s.title}</p>
            <p className="truncate text-xs text-white/45">
              {s.artist ?? "Unknown"}
            </p>
          </div>
          <span className="shrink-0 rounded-full bg-white/10 px-2 py-0.5 text-xs tabular-nums">
            {s.count} {unit}
            {s.count === 1 ? "" : "s"}
          </span>
        </li>
      ))}
    </ol>
  );
}

/** Preset ban lengths; 0 is permanent. */
const BAN_OPTIONS: { label: string; minutes: number }[] = [
  { label: "15 min", minutes: 15 },
  { label: "1 h", minutes: 60 },
  { label: "24 h", minutes: 60 * 24 },
  { label: "Forever", minutes: 0 },
];

/** "42 min left" / "3 h left" until a ban lifts. */
function banLeft(until: string): string {
  const mins = Math.max(
    1,
    Math.ceil((new Date(until).getTime() - Date.now()) / 60_000),
  );
  if (mins < 60) return `${mins} min left`;
  if (mins < 48 * 60) return `${Math.round(mins / 60)} h left`;
  return `${Math.round(mins / (60 * 24))} days left`;
}

function Guests({
  items,
  onChange,
  onError,
  isAdmin,
}: {
  items: UserStat[];
  onChange: () => void;
  onError: (msg: string) => void;
  isAdmin: boolean;
}): JSX.Element {
  // IP of the row currently showing its ban-length choices.
  const [picking, setPicking] = useState<string | null>(null);

  const fail = (e: unknown): void =>
    onError(e instanceof Error ? e.message : String(e));
  const run = (p: Promise<unknown>): void => {
    p.then(() => {
      setPicking(null);
      onChange();
    }).catch(fail);
  };

  if (items.length === 0) return <Empty text="No guest activity yet." />;
  return (
    <ul className="max-h-80 space-y-1 overflow-y-auto pr-1">
      {items.map((u) => {
        // Null for guests: only admins are sent addresses, and only they may ban.
        const ip = u.ip;
        return (
          <li key={u.id} className="flex items-center gap-2 text-sm">
            <div className="min-w-0 flex-1">
              <p
                className={`truncate ${u.banned ? "text-white/45 line-through" : ""}`}
              >
                {u.name ?? u.ip ?? "Guest"}
              </p>
              <p className="truncate text-xs text-white/35">
                {u.name && u.ip ? `${u.ip} · ` : ""}
                {u.requests} added · {u.downvotes} downvoted
                {u.banned && (
                  <span className="text-red-300/80">
                    {" · "}
                    {u.bannedUntil ? banLeft(u.bannedUntil) : "blocked"}
                  </span>
                )}
              </p>
            </div>

            {!isAdmin || !ip ? null : u.banned ? (
              <button
                onClick={() => run(api.unbanGuest(ip))}
                className="shrink-0 rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-white/15 hover:text-white"
              >
                Unblock
              </button>
            ) : picking === ip ? (
              <div className="flex shrink-0 items-center gap-1">
                {BAN_OPTIONS.map((o) => (
                  <button
                    key={o.label}
                    onClick={() => run(api.banGuest(ip, o.minutes))}
                    className="rounded-md bg-red-500/20 px-1.5 py-0.5 text-xs text-red-200 hover:bg-red-500/35"
                  >
                    {o.label}
                  </button>
                ))}
                <button
                  onClick={() => setPicking(null)}
                  className="rounded-md px-1 py-0.5 text-xs text-white/40 hover:text-white"
                  aria-label="Cancel"
                >
                  ✕
                </button>
              </div>
            ) : (
              <button
                onClick={() => setPicking(ip)}
                className="shrink-0 rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
                title="Block this guest from adding songs"
              >
                Block
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}
