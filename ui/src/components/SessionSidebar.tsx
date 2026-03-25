import { useCallback, useEffect, useRef, useState } from "react";
import type { MutableRefObject } from "react";
import { getVersion } from "@tauri-apps/api/app";
import type { Update } from "../lib/updater";
import { checkForUpdate, downloadAndInstall } from "../lib/updater";
import { listSessions, cleanSessions, getUpdateReadiness } from "../lib/commands";
import type { Session, UpdateReadiness } from "../types";
import { PhaseBadge } from "./PhaseBadge";
import { formatLocalTime } from "../lib/format";

type UpdateState = "available" | "downloading" | "error";

function Spinner({ color = "border-gray-400" }: { color?: string }) {
  return (
    <span className={`inline-block w-3 h-3 rounded-full border-2 border-t-transparent animate-spin ${color}`} />
  );
}

interface SessionSidebarProps {
  selectedId: string | null;
  onSelect: (session: Session) => void;
  onNewSession: () => void;
  onRunAll: () => void;
  onRefreshRef?: MutableRefObject<(() => void) | null>;
  /** Called after each load() when the currently selected session appears in
   *  the result, passing the latest DTO so the parent can stay in sync without
   *  triggering a view-change side effect (i.e. never call onSelect here). */
  onSelectedSessionUpdated?: (session: Session) => void;
}

export function SessionSidebar({ selectedId, onSelect, onNewSession, onRunAll, onRefreshRef, onSelectedSessionUpdated: onSelectedSessionUpdatedProp }: SessionSidebarProps) {
  // Stable refs so load() can access the latest props without re-creating itself
  const onSelectedSessionUpdatedRef = useRef(onSelectedSessionUpdatedProp);
  onSelectedSessionUpdatedRef.current = onSelectedSessionUpdatedProp;
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleanMessage, setCleanMessage] = useState<string | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [update, setUpdate] = useState<Update | null>(null);
  const [updateState, setUpdateState] = useState<UpdateState>("available");
  const [updateReadiness, setUpdateReadiness] = useState<UpdateReadiness | null>(null);
  const [errorMsg, setErrorMsg] = useState("");
  const lastFingerprintRef = useRef("");
  const inflightRef = useRef(false);

  const load = useCallback(async (silent = false) => {
    if (inflightRef.current) return;
    inflightRef.current = true;
    if (!silent) {
      setLoading(true);
    }
    try {
      const fetched = await listSessions();
      const sorted = [...fetched].sort((a, b) => {
        const aInput = a.awaitingInput || a.phase === "Awaiting Approval";
        const bInput = b.awaitingInput || b.phase === "Awaiting Approval";
        if (aInput !== bInput) return aInput ? -1 : 1;
        const aTime = a.updatedAt ?? a.createdAt;
        const bTime = b.updatedAt ?? b.createdAt;
        return bTime.localeCompare(aTime);
      });
      const fingerprint = sorted.map(s => `${s.id}:${s.phase}:${s.updatedAt ?? s.createdAt}:${!!s.awaitingInput}:${!!s.planAvailable}`).join(",");
      if (fingerprint !== lastFingerprintRef.current) {
        lastFingerprintRef.current = fingerprint;
        setSessions(sorted);
        if (selectedIdRef.current !== null) {
          const match = sorted.find((s) => s.id === selectedIdRef.current);
          if (match) {
            onSelectedSessionUpdatedRef.current?.(match);
          }
        }
      }
      setError(null);
    } catch (e) {
      if (!silent) {
        setError(String(e));
      }
    } finally {
      inflightRef.current = false;
      if (!silent) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const doSilentLoad = () => {
      if (document.visibilityState === "visible") {
        void load(true);
      }
    };
    const interval = setInterval(doSilentLoad, 3000);
    document.addEventListener("visibilitychange", doSilentLoad);
    return () => {
      clearInterval(interval);
      document.removeEventListener("visibilitychange", doSilentLoad);
    };
  }, [load]);

  useEffect(() => {
    if (onRefreshRef) {
      onRefreshRef.current = () => void load(true);
      return () => { onRefreshRef.current = null; };
    }
  }, [load, onRefreshRef]);

  useEffect(() => {
    let updateIntervalId: ReturnType<typeof setInterval>;

    void getVersion().then(setVersion).catch(() => {});

    void getUpdateReadiness().then(setUpdateReadiness).catch(() => {});

    const doCheck = () => {
      void checkForUpdate().then((u) => { if (u) setUpdate(u); });
    };

    const updateTimerId = setTimeout(() => {
      void getUpdateReadiness().then(setUpdateReadiness).catch(() => {});
      doCheck();
      updateIntervalId = setInterval(doCheck, 24 * 60 * 60 * 1000);
    }, 2000);

    return () => {
      clearTimeout(updateTimerId);
      clearInterval(updateIntervalId);
    };
  }, []);

  async function handleInstall() {
    setUpdateState("downloading");
    try {
      await downloadAndInstall(update!);
    } catch (e) {
      setUpdateState("error");
      setErrorMsg(String(e));
    }
  }

  async function handleClean() {
    setCleaning(true);
    setCleanMessage(null);
    try {
      const result = await cleanSessions();
      setCleanMessage(`${result.deleted} deleted (skipped: ${result.skipped})`);
      void load(true);
    } catch (e) {
      setCleanMessage(`Error: ${e}`);
    } finally {
      setCleaning(false);
    }
  }

  const showAutoUpdate = update && updateState === "available" && updateReadiness?.canAutoUpdate;
  const updateGuidance = updateReadiness && !updateReadiness.canAutoUpdate ? updateReadiness.guidance : null;

  return (
    <div className="h-full flex flex-col">
      <div className="px-3 py-3 border-b border-gray-800 space-y-1.5">
        <div className="flex items-center justify-between gap-2">
          <h2 className="text-sm font-semibold text-gray-200">Sessions</h2>
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={() => void handleClean()}
              disabled={cleaning}
              className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded disabled:opacity-50 flex items-center gap-1"
              title="Clean completed sessions"
            >
              {cleaning ? (
                <>
                  <Spinner />
                  Cleaning...
                </>
              ) : (
                "Clean"
              )}
            </button>
            <button
              type="button"
              onClick={onRunAll}
              disabled={!sessions.some((s) => s.phase === "Planned" || s.phase === "Suspended")}
              className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded disabled:opacity-50"
              title="Run all pending sessions"
            >
              Run All
            </button>
            <button
              type="button"
              onClick={onNewSession}
              className="px-2 py-1 text-xs bg-blue-600 text-white hover:bg-blue-700 rounded"
            >
              + New
            </button>
          </div>
        </div>
        {cleanMessage && (
          <p className="text-xs text-gray-400">{cleanMessage}</p>
        )}
      </div>

      <div className="flex-1 overflow-y-auto">
        {loading && (
          <p className="p-3 text-xs text-gray-500">Loading...</p>
        )}
        {error && (
          <p className="p-3 text-xs text-red-400">Error: {error}</p>
        )}
        {!loading && !error && sessions.length === 0 && (
          <p className="p-3 text-xs text-gray-500">No sessions found.</p>
        )}
        {sessions.map((s) => (
          <button
            key={s.id}
            type="button"
            onClick={() => onSelect(s)}
            className={`w-full text-left px-3 py-2.5 border-b border-gray-800/50 hover:bg-gray-800 transition-colors ${
              selectedId === s.id ? "bg-gray-800" : ""
            }`}
          >
            <div className="flex items-center justify-between gap-2 mb-0.5">
              <span className="text-xs text-gray-500 font-mono truncate">{s.id}</span>
              <PhaseBadge phase={s.phase} planAvailable={s.planAvailable} />
            </div>
            <p className="text-sm text-gray-300 truncate">{s.title || s.input}</p>
            {s.title && (
              <p className="text-xs text-gray-500 truncate">{s.input}</p>
            )}
            <div className="flex items-center gap-1.5 mt-0.5">
              <span className="text-xs text-blue-400/70 font-mono truncate">
                {s.baseDir.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? s.baseDir}
              </span>
              <span className="text-xs text-gray-600">{formatLocalTime(s.updatedAt ?? s.createdAt)}</span>
            </div>
          </button>
        ))}
      </div>

      {/* Sidebar footer: version & update */}
      <div className="flex-shrink-0 border-t border-gray-800 px-3 py-2">
        <div className="text-xs text-gray-500">{version ? `v${version}` : "…"}</div>
        {showAutoUpdate && (
          <div className="mt-1 space-y-1">
            <div className="text-xs text-green-400">v{update.version} available</div>
            <button
              type="button"
              onClick={() => void handleInstall()}
              className="px-2 py-0.5 bg-blue-600 text-white rounded text-xs hover:bg-blue-700"
            >
              Update
            </button>
          </div>
        )}
        {updateGuidance && (
          <div className="mt-1 text-xs text-yellow-400">{updateGuidance}</div>
        )}
        {updateState === "downloading" && (
          <div className="mt-1 text-xs text-gray-400">Downloading…</div>
        )}
        {updateState === "error" && (
          <div className="mt-1 space-y-1">
            <div className="text-xs text-red-400">{errorMsg}</div>
            <button
              type="button"
              onClick={() => { setUpdate(null); setUpdateState("available"); setErrorMsg(""); }}
              className="px-2 py-0.5 border border-gray-700 text-gray-400 rounded text-xs hover:bg-gray-800"
            >
              Dismiss
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
