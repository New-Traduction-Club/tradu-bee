import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { strings } from "./strings";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  HashRouter,
  Navigate,
  NavLink,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom";
import OobeSetupView from "./components/OobeSetupView";
import type {
  InstallationEvent,
  InstallationProgressEvent,
  InstalledMod,
  LauncherStateView,
  ModProcessStatusEvent,
  SupportedMod,
} from "./types/launcher";

interface UiInstallationTask extends InstallationProgressEvent {
  updatedAtEpochMs: number;
}

function App() {
  return (
    <HashRouter>
      <LauncherClient />
    </HashRouter>
  );
}

function LauncherClient() {
  const navigate = useNavigate();
  const location = useLocation();

  const [launcherState, setLauncherState] = useState<LauncherStateView | null>(
    null,
  );
  const [catalog, setCatalog] = useState<SupportedMod[]>([]);
  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);
  const [manualArchiveBySlug, setManualArchiveBySlug] = useState<
    Record<string, string>
  >({});
  const [statusMessage, setStatusMessage] = useState("");
  const [loadingState, setLoadingState] = useState(true);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [installRequestSlug, setInstallRequestSlug] = useState<string | null>(
    null,
  );
  const [uninstallingSlug, setUninstallingSlug] = useState<string | null>(null);
  const [tasksBySlug, setTasksBySlug] = useState<Record<string, UiInstallationTask>>(
    {},
  );
  const [runningProcessSlugs, setRunningProcessSlugs] = useState<Set<string>>(
    new Set(),
  );

  const installedBySlug = useMemo(() => {
    const map = new Map<string, InstalledMod>();
    for (const installed of launcherState?.installedMods ?? []) {
      map.set(installed.slug, installed);
    }
    return map;
  }, [launcherState?.installedMods]);

  const catalogBySlug = useMemo(() => {
    const map = new Map<string, SupportedMod>();
    for (const mod of catalog) {
      map.set(mod.slug, mod);
    }
    return map;
  }, [catalog]);

  const installedCatalog = useMemo(() => {
    return (launcherState?.installedMods ?? []).map((installed) => {
      return catalogBySlug.get(installed.slug) ?? fallbackMod(installed);
    });
  }, [catalogBySlug, launcherState?.installedMods]);

  const selectedExploreMod = useMemo(
    () => catalog.find((mod) => mod.slug === selectedSlug) ?? catalog[0] ?? null,
    [catalog, selectedSlug],
  );

  const progressTasks = useMemo(
    () =>
      Object.values(tasksBySlug).sort(
        (left, right) => right.updatedAtEpochMs - left.updatedAtEpochMs,
      ),
    [tasksBySlug],
  );

  const runningInstallationSlugs = useMemo(() => {
    const active = new Set<string>();
    for (const task of Object.values(tasksBySlug)) {
      if (task.state === "queued" || task.state === "running") {
        active.add(task.slug);
      }
    }
    return active;
  }, [tasksBySlug]);

  async function refreshLauncherState(): Promise<LauncherStateView> {
    const state = await invoke<LauncherStateView>("get_launcher_state");
    setLauncherState(state);
    return state;
  }

  async function refreshCatalog(silent = false) {
    if (!silent) {
      setLoadingCatalog(true);
      setStatusMessage("Actualizando catálogo remoto...");
    }

    try {
      const mods = await invoke<SupportedMod[]>("fetch_supported_mods");
      setCatalog(mods);
      setSelectedSlug((current) => {
        if (current && mods.some((mod) => mod.slug === current)) {
          return current;
        }
        return mods[0]?.slug ?? null;
      });
      if (!silent) {
        setStatusMessage(`Catálogo actualizado: ${mods.length} mods disponibles.`);
      }
    } catch (error) {
      if (!silent) {
        setStatusMessage(String(error));
      }
    } finally {
      if (!silent) {
        setLoadingCatalog(false);
      }
    }
  }

  async function queueInstallation(mod: SupportedMod) {
    const manualArchivePath = manualArchiveBySlug[mod.slug] ?? null;
    if (!mod.downloadable && !manualArchivePath) {
      setStatusMessage(strings.manualRequired);
      return;
    }

    setInstallRequestSlug(mod.slug);
    try {
      await invoke("execute_installation_recipe", {
        slug: mod.slug,
        userProvidedZipPath: mod.downloadable ? null : manualArchivePath,
      });
      setStatusMessage(`${strings.backgroundInstallStarted} ${mod.name}.`);
    } catch (error) {
      setStatusMessage(String(error));
    } finally {
      setInstallRequestSlug(null);
    }
  }

  async function cancelInstallation(slug: string) {
    try {
      await invoke("cancel_installation", { slug });
      setStatusMessage(`${strings.cancelling} (${slug})`);
    } catch (error) {
      setStatusMessage(String(error));
    }
  }

  async function uninstallInstalledMod(slug: string) {
    setUninstallingSlug(slug);
    setStatusMessage(`${strings.uninstalling} ${slug}...`);
    try {
      await invoke("uninstall_mod", { slug });
      await refreshLauncherState();
      setStatusMessage(`${strings.uninstallCompleted}: ${slug}`);
    } catch (error) {
      setStatusMessage(String(error));
    } finally {
      setUninstallingSlug(null);
    }
  }

  async function launchInstalledMod(slug: string) {
    if (runningProcessSlugs.has(slug)) {
      setStatusMessage(`${slug} ya está en ejecución.`);
      return;
    }

    try {
      await invoke("launch_installed_mod", { slug });
      setStatusMessage(`Iniciando ${slug}...`);
    } catch (error) {
      setStatusMessage(String(error));
    }
  }

  async function openManualDownload(mod: SupportedMod) {
    const url = mod.downloadUrl?.trim();
    if (!url) {
      setStatusMessage("El mod no tiene URL de descarga manual disponible.");
      return;
    }

    try {
      await openExternal(url);
      setStatusMessage(`Se abrió el navegador para descargar ${mod.name}.`);
    } catch (error) {
      setStatusMessage(String(error));
    }
  }

  async function selectManualArchive(slug: string) {
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "Archivo de mod", extensions: ["zip", "rar"] }],
      });
      if (typeof selected !== "string") {
        return;
      }
      setManualArchiveBySlug((current) => ({ ...current, [slug]: selected }));
      setStatusMessage(`Archivo seleccionado para ${slug}.`);
    } catch (error) {
      setStatusMessage(String(error));
    }
  }

  useEffect(() => {
    let mounted = true;
    let unlistenStatus: (() => void) | null = null;
    let unlistenProgress: (() => void) | null = null;
    let unlistenModProcess: (() => void) | null = null;
    let runningPollTimer: ReturnType<typeof window.setInterval> | null = null;

    const syncRunningProcesses = async (silent = true) => {
      try {
        const running = await invoke<string[]>("get_running_mod_processes");
        if (!mounted) {
          return;
        }
        setRunningProcessSlugs(new Set(running));
      } catch (error) {
        if (!silent && mounted) {
          setStatusMessage(String(error));
        }
      }
    };

    const handleFocus = () => {
      void syncRunningProcesses(true);
    };
    window.addEventListener("focus", handleFocus);

    void (async () => {
      try {
        const state = await refreshLauncherState();
        if (!mounted) {
          return;
        }

        await syncRunningProcesses(false);

        if (state.oobeCompleted) {
          await refreshCatalog(true);
        }
      } catch (error) {
        if (mounted) {
          setStatusMessage(String(error));
        }
      } finally {
        if (mounted) {
          setLoadingState(false);
        }
      }

      unlistenStatus = await listen<InstallationEvent>(
        "installation-status",
        (event) => {
          if (!mounted) {
            return;
          }
          const payload = event.payload;
          setStatusMessage(`[${payload.slug}] ${payload.message}`);
          if (payload.status === "success" || payload.status === "uninstalled") {
            void refreshLauncherState();
          }
        },
      );

      unlistenProgress = await listen<InstallationProgressEvent>(
        "installation-progress",
        (event) => {
          if (!mounted) {
            return;
          }
          const payload = event.payload;
          setTasksBySlug((current) => ({
            ...current,
            [payload.slug]: {
              ...payload,
              updatedAtEpochMs: Date.now(),
            },
          }));

          if (payload.state === "success") {
            void refreshLauncherState();
          }

          if (payload.state === "success" || payload.state === "failed" || payload.state === "cancelled") {
            const slug = payload.slug;
            window.setTimeout(() => {
              setTasksBySlug((current) => {
                if (!current[slug] || current[slug].state === "running" || current[slug].state === "queued") {
                  return current;
                }
                const next = { ...current };
                delete next[slug];
                return next;
              });
            }, 8000);
          }
        },
      );

      unlistenModProcess = await listen<ModProcessStatusEvent>(
        "mod-process-status",
        (event) => {
          if (!mounted) {
            return;
          }
          const payload = event.payload;
          setRunningProcessSlugs((current) => {
            const next = new Set(current);
            if (payload.isRunning) {
              next.add(payload.slug);
            } else {
              next.delete(payload.slug);
            }
            return next;
          });
        },
      );

      runningPollTimer = window.setInterval(() => {
        void syncRunningProcesses();
      }, 20000);
    })();

    return () => {
      mounted = false;
      if (unlistenStatus) {
        unlistenStatus();
      }
      if (unlistenProgress) {
        unlistenProgress();
      }
      if (unlistenModProcess) {
        unlistenModProcess();
      }
      if (runningPollTimer) {
        window.clearInterval(runningPollTimer);
      }
      window.removeEventListener("focus", handleFocus);
    };
  }, []);

  useEffect(() => {
    if (!launcherState) {
      return;
    }
    if (!launcherState.oobeCompleted && location.pathname !== "/setup") {
      navigate("/setup", { replace: true });
      return;
    }
    if (launcherState.oobeCompleted && location.pathname === "/setup") {
      navigate("/explore", { replace: true });
    }
  }, [launcherState, location.pathname, navigate]);

  if (loadingState || !launcherState) {
    return (
      <div className="flex h-screen items-center justify-center bg-slate-950 text-slate-200">
        Cargando launcher...
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/95">
        <div className="flex h-14 items-center justify-between px-5">
          <nav className="flex items-center gap-2">
            <NavLink to="/library" className={topNavigationLinkClassName}>
              Biblioteca
            </NavLink>
            <NavLink to="/explore" className={topNavigationLinkClassName}>
              Explorar mods
            </NavLink>
            <NavLink to="/settings" className={topNavigationLinkClassName}>
              Ajustes
            </NavLink>
          </nav>
          <div className="text-xs text-slate-400">Tradu-Bee Launcher</div>
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-hidden bg-[#0f172a]">
        <Routes>
          <Route
            path="/setup"
            element={
              <OobeSetupView
                expectedHash={launcherState.expectedDdlcSha256}
                initialOriginalZipPath={launcherState.cachedDdlcZipPath}
                initialInstallDir={launcherState.globalInstallDir}
                onCompleted={(state) => {
                  setLauncherState(state);
                  void refreshCatalog(true);
                  navigate("/explore", { replace: true });
                }}
                onStatus={setStatusMessage}
              />
            }
          />
          <Route
            path="/library"
            element={
              <LibraryRoute
                mods={installedCatalog}
                selectedSlug={selectedSlug}
                runningProcessSlugs={runningProcessSlugs}
                onSelect={setSelectedSlug}
                onPlay={launchInstalledMod}
                onUninstall={uninstallInstalledMod}
                uninstallingSlug={uninstallingSlug}
              />
            }
          />
          <Route
            path="/explore"
            element={
              <ExploreRoute
                mods={catalog}
                selectedMod={selectedExploreMod}
                selectedSlug={selectedSlug}
                manualArchiveBySlug={manualArchiveBySlug}
                installedBySlug={installedBySlug}
                runningProcessSlugs={runningProcessSlugs}
                installRequestSlug={installRequestSlug}
                uninstallingSlug={uninstallingSlug}
                loadingCatalog={loadingCatalog}
                runningInstallationSlugs={runningInstallationSlugs}
                onSelect={setSelectedSlug}
                onPlay={launchInstalledMod}
                onInstall={queueInstallation}
                onUninstall={uninstallInstalledMod}
                onRefreshCatalog={() => void refreshCatalog()}
                onOpenManualDownload={openManualDownload}
                onSelectManualArchive={selectManualArchive}
              />
            }
          />
          <Route path="/settings" element={<SettingsRoute />} />
          <Route
            path="*"
            element={
              <Navigate
                to={launcherState.oobeCompleted ? "/explore" : "/setup"}
                replace
              />
            }
          />
        </Routes>
      </main>

      <GlobalProgressFooter tasks={progressTasks} statusMessage={statusMessage} onCancel={cancelInstallation} />
    </div>
  );
}

interface ExploreRouteProps {
  mods: SupportedMod[];
  selectedMod: SupportedMod | null;
  selectedSlug: string | null;
  manualArchiveBySlug: Record<string, string>;
  installedBySlug: Map<string, InstalledMod>;
  runningProcessSlugs: Set<string>;
  installRequestSlug: string | null;
  uninstallingSlug: string | null;
  loadingCatalog: boolean;
  runningInstallationSlugs: Set<string>;
  onSelect: (slug: string) => void;
  onPlay: (slug: string) => Promise<void>;
  onInstall: (mod: SupportedMod) => Promise<void>;
  onUninstall: (slug: string) => Promise<void>;
  onRefreshCatalog: () => void;
  onOpenManualDownload: (mod: SupportedMod) => Promise<void>;
  onSelectManualArchive: (slug: string) => Promise<void>;
}

function ExploreRoute({
  mods,
  selectedMod,
  selectedSlug,
  manualArchiveBySlug,
  installedBySlug,
  runningProcessSlugs,
  installRequestSlug,
  uninstallingSlug,
  loadingCatalog,
  runningInstallationSlugs,
  onSelect,
  onPlay,
  onInstall,
  onUninstall,
  onRefreshCatalog,
  onOpenManualDownload,
  onSelectManualArchive,
}: ExploreRouteProps) {
  const selectedInstalled = selectedMod
    ? installedBySlug.get(selectedMod.slug) ?? null
    : null;
  const selectedIsRunning = selectedMod
    ? runningProcessSlugs.has(selectedMod.slug)
    : false;

  return (
    <div className="flex h-full min-h-0">
      <div className="w-[320px] shrink-0 border-r border-slate-800 bg-slate-900">
        <div className="flex items-center justify-between border-b border-slate-800 px-4 py-3">
          <p className="text-xs uppercase tracking-wide text-slate-400">Explorar mods</p>
          <button
            type="button"
            onClick={onRefreshCatalog}
            className="rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 transition hover:border-slate-500 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={loadingCatalog}
          >
            {loadingCatalog ? "Actualizando..." : "Actualizar"}
          </button>
        </div>
        <div className="h-[calc(100%-49px)] overflow-y-auto p-3">
          <div className="space-y-2">
            {mods.map((mod) => {
              const installed = installedBySlug.has(mod.slug);
              const selected = selectedSlug === mod.slug;
              return (
                <button
                  key={mod.slug}
                  type="button"
                  onClick={() => onSelect(mod.slug)}
                  className={`w-full rounded-lg border p-2 text-left transition ${selected
                    ? "border-yellow-500/60 bg-slate-800"
                    : "border-slate-800 bg-slate-950 hover:bg-slate-800/50"
                    }`}
                >
                  <div className="flex items-center gap-3">
                    {mod.logoImageUrl ? (
                      <img
                        src={mod.logoImageUrl}
                        alt={`${mod.name} logo`}
                        className="h-10 w-10 rounded-md object-cover"
                      />
                    ) : (
                      <div className="h-10 w-10 rounded-md bg-slate-700" />
                    )}
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium">{mod.name}</p>
                      <p className="truncate text-xs text-slate-400">
                        {installed ? "Instalado" : "No instalado"} ·{" "}
                        {mod.status || "Sin estado"}
                      </p>
                    </div>
                  </div>
                </button>
              );
            })}
            {mods.length === 0 && (
              <p className="rounded-lg border border-dashed border-slate-700 p-4 text-sm text-slate-400">
                Sin mods disponibles.
              </p>
            )}
          </div>
        </div>
      </div>

      <div className="min-w-0 flex-1 overflow-y-auto">
        {!selectedMod ? (
          <div className="flex h-full items-center justify-center text-slate-400">
            Selecciona un mod para ver detalles.
          </div>
        ) : (
          <div className="pb-8">
            <ModHeroSection mod={selectedMod} />

            <section className="-mt-7 px-6">
              <div className="rounded-xl border border-slate-800 bg-slate-900 p-4 shadow-2xl">
                <h3 className="text-sm font-semibold text-slate-200"></h3>
                <div className="mt-3 flex flex-wrap items-center gap-3">
                  {selectedInstalled ? (
                    <>
                      <button
                        type="button"
                        onClick={() => void onPlay(selectedMod.slug)}
                        className={`rounded-lg px-5 py-2.5 text-sm font-semibold transition ${selectedIsRunning
                          ? "cursor-not-allowed border border-emerald-400/40 bg-emerald-500/10 text-emerald-300"
                          : "bg-gradient-to-r from-yellow-500 to-orange-500 text-slate-950 hover:brightness-110"
                          }`}
                        disabled={selectedIsRunning}
                      >
                        {selectedIsRunning ? strings.playing : strings.play}
                      </button>
                      <button
                        type="button"
                        onClick={() => void onUninstall(selectedMod.slug)}
                        className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2.5 text-sm font-medium text-slate-100 transition hover:border-slate-600 disabled:cursor-not-allowed disabled:opacity-60"
                        disabled={uninstallingSlug === selectedMod.slug || selectedIsRunning}
                        title={selectedIsRunning ? strings.uninstallRequiredWarn : undefined}
                      >
                        {uninstallingSlug === selectedMod.slug
                          ? strings.uninstalling
                          : strings.uninstall}
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void onInstall(selectedMod)}
                      className="rounded-lg bg-gradient-to-r from-yellow-500 to-orange-500 px-5 py-2.5 text-sm font-semibold text-slate-950 transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
                      disabled={
                        installRequestSlug === selectedMod.slug ||
                        runningInstallationSlugs.has(selectedMod.slug) ||
                        (!selectedMod.downloadable &&
                          !manualArchiveBySlug[selectedMod.slug])
                      }
                    >
                      {installRequestSlug === selectedMod.slug ||
                        runningInstallationSlugs.has(selectedMod.slug)
                        ? strings.processing
                        : strings.install}
                    </button>
                  )}

                  {!selectedInstalled && (
                    <details className="mt-4 w-full text-slate-400 group">
                      <summary className="text-xs font-semibold cursor-pointer select-none text-slate-300 hover:text-white transition flex items-center gap-1.5 focus:outline-none">
                        <span className="inline-block transform transition-transform duration-200 group-open:rotate-90">▶</span>
                        {strings.advancedOptions}
                      </summary>
                      <div className="mt-3 border border-slate-800 bg-slate-950/60 rounded-lg p-3 grid gap-3">
                        <div className="flex flex-wrap gap-2">
                          <button
                            type="button"
                            onClick={() => void onOpenManualDownload(selectedMod)}
                            className={
                              selectedMod.downloadable
                                ? "rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs font-medium text-slate-100 transition hover:border-slate-600"
                                : "rounded-lg border border-amber-500/50 bg-amber-500/10 px-3 py-1.5 text-xs font-medium text-amber-200 transition hover:border-amber-400"
                            }
                          >
                            {strings.manualDownload}
                          </button>
                          <button
                            type="button"
                            onClick={() => void onSelectManualArchive(selectedMod.slug)}
                            className="rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs font-medium text-slate-100 transition hover:border-slate-600"
                          >
                            {manualArchiveBySlug[selectedMod.slug] ? strings.changeFile : strings.selectFile}
                          </button>
                        </div>
                        <p className="text-xs text-slate-400">
                          {selectedMod.downloadable ? (
                            manualArchiveBySlug[selectedMod.slug] ? (
                              <span className="text-emerald-400 font-medium">
                                ✓ {strings.localSelected}: {manualArchiveBySlug[selectedMod.slug]}
                              </span>
                            ) : (
                              strings.autoDownloadInfo
                            )
                          ) : (
                            <span className="text-amber-300 font-medium">
                              {manualArchiveBySlug[selectedMod.slug]
                                ? `Archivo seleccionado: ${manualArchiveBySlug[selectedMod.slug]}`
                                : strings.manualRequired}
                            </span>
                          )}
                        </p>
                      </div>
                    </details>
                  )}
                </div>
              </div>
            </section>

            <section className="grid gap-6 px-6 pt-6">
              <ModDescriptionSection mod={selectedMod} />
              <CreditsSection mod={selectedMod} />
              <ScreenshotsSection mod={selectedMod} />
            </section>
          </div>
        )}
      </div>
    </div>
  );
}

interface LibraryRouteProps {
  mods: SupportedMod[];
  selectedSlug: string | null;
  runningProcessSlugs: Set<string>;
  onSelect: (slug: string) => void;
  onPlay: (slug: string) => Promise<void>;
  onUninstall: (slug: string) => Promise<void>;
  uninstallingSlug: string | null;
}

function LibraryRoute({
  mods,
  selectedSlug,
  runningProcessSlugs,
  onSelect,
  onPlay,
  onUninstall,
  uninstallingSlug,
}: LibraryRouteProps) {
  const selected = useMemo(
    () => mods.find((mod) => mod.slug === selectedSlug) ?? mods[0] ?? null,
    [mods, selectedSlug],
  );
  const selectedIsRunning = selected ? runningProcessSlugs.has(selected.slug) : false;

  return (
    <div className="flex h-full min-h-0">
      <div className="w-[320px] shrink-0 border-r border-slate-800 bg-slate-900">
        <div className="border-b border-slate-800 px-4 py-3 text-xs uppercase tracking-wide text-slate-400">
          Biblioteca
        </div>
        <div className="h-[calc(100%-43px)] overflow-y-auto p-3">
          <div className="space-y-2">
            {mods.map((mod) => {
              const isSelected = mod.slug === selected?.slug;
              return (
                <button
                  key={mod.slug}
                  type="button"
                  onClick={() => onSelect(mod.slug)}
                  onDoubleClick={() => void onPlay(mod.slug)}
                  className={`w-full rounded-lg border p-2 text-left transition ${isSelected
                    ? "border-yellow-500/60 bg-slate-800"
                    : "border-slate-800 bg-slate-950 hover:bg-slate-800/50"
                    }`}
                >
                  <div className="flex items-center gap-3">
                    {mod.logoImageUrl ? (
                      <img
                        src={mod.logoImageUrl}
                        alt={`${mod.name} logo`}
                        className="h-10 w-10 rounded-md object-cover"
                      />
                    ) : (
                      <div className="h-10 w-10 rounded-md bg-slate-700 shrink-0" />
                    )}
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm font-medium">{mod.name}</p>
                    </div>
                  </div>
                </button>
              );
            })}
            {mods.length === 0 && (
              <p className="rounded-lg border border-dashed border-slate-700 p-4 text-sm text-slate-400">
                Todavía no hay mods instalados.
              </p>
            )}
          </div>
        </div>
      </div>

      <div className="min-w-0 flex-1 overflow-y-auto">
        {!selected ? (
          <div className="flex h-full items-center justify-center text-slate-400">
          </div>
        ) : (
          <div className="pb-8">
            <ModHeroSection mod={selected} />
            <section className="-mt-7 px-6">
              <div className="rounded-xl border border-slate-800 bg-slate-900 p-4 shadow-2xl">
                <br></br>
                <div className="mt-3 flex gap-3">
                  <button
                    type="button"
                    onClick={() => void onPlay(selected.slug)}
                    className={`rounded-lg px-5 py-2.5 text-sm font-semibold transition ${selectedIsRunning
                      ? "cursor-not-allowed border border-emerald-400/40 bg-emerald-500/10 text-emerald-300"
                      : "bg-gradient-to-r from-yellow-500 to-orange-500 text-slate-950 hover:brightness-110"
                      }`}
                    disabled={selectedIsRunning}
                  >
                    {selectedIsRunning ? "Ejecutando" : "Jugar"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void onUninstall(selected.slug)}
                    className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2.5 text-sm font-medium text-slate-100 transition hover:border-slate-600 disabled:cursor-not-allowed disabled:opacity-60"
                    disabled={uninstallingSlug === selected.slug}
                  >
                    {uninstallingSlug === selected.slug
                      ? "Desinstalando..."
                      : "Desinstalar"}
                  </button>
                </div>
              </div>
            </section>
            <section className="grid gap-6 px-6 pt-6">
              <ModDescriptionSection mod={selected} />
              <CreditsSection mod={selected} />
              <ScreenshotsSection mod={selected} />
            </section>
          </div>
        )}
      </div>
    </div>
  );
}

function SettingsRoute() {
  return (
    <div className="flex h-full items-center justify-center bg-[#0f172a] px-6">
      <div className="w-full max-w-3xl rounded-xl border border-slate-800 bg-slate-900 p-8">
        <h2 className="text-xl font-semibold text-slate-100">Ajustes</h2>
        <p className="mt-2 text-sm text-slate-400">
          Próximamente haré algo XD.
        </p>
      </div>
    </div>
  );
}

function ModHeroSection({ mod }: { mod: SupportedMod }) {
  return (
    <section className="relative h-120 border-b border-slate-800 bg-slate-950 overflow-hidden">
      {mod.heroImageUrl ? (
        <>
          <img
            src={mod.heroImageUrl}
            alt=""
            className="absolute inset-0 h-full w-full object-cover blur-xl opacity-25 scale-110 pointer-events-none"
          />
          <div className="absolute inset-0 bg-gradient-to-t from-slate-950 via-slate-950/40 to-transparent z-0" />
          <div className="absolute inset-0 flex items-center justify-center z-10 py-2">
            <img
              src={mod.heroImageUrl}
              alt={`${mod.name} portada`}
              className="h-full max-w-full object-contain rounded-lg shadow-2xl"
            />
          </div>
        </>
      ) : (
        <div className="absolute inset-0 bg-slate-900" />
      )}

      <div className="absolute inset-0 bg-gradient-to-t from-slate-950/80 via-transparent to-transparent z-20 pointer-events-none" />

      <div className="absolute inset-x-0 bottom-0 p-6 z-30">
        <div className="flex items-end gap-4">
          {mod.logoImageUrl && (
            <img
              src={mod.logoImageUrl}
              alt={`${mod.name} logo`}
              className="h-20 w-20 rounded-xl border border-slate-700 bg-slate-900 object-cover shadow-lg"
            />
          )}
          <div className="drop-shadow-lg">
            <span
              className={`inline-flex rounded-full border px-3 py-1 text-xs font-semibold ${statusBadgeClasses(mod.status)}`}
            >
              {mod.status || "Sin estado"}
            </span>
            <h2 className="mt-3 text-3xl font-bold tracking-tight text-white">
              {mod.name}
            </h2>
          </div>
        </div>
      </div>
    </section>
  );
}

function ModDescriptionSection({ mod }: { mod: SupportedMod }) {
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900 p-5">
      <h3 className="text-sm font-semibold text-slate-200">Descripción y metadata</h3>
      <div
        className="prose prose-invert prose-slate mt-4 max-w-none text-sm"
        dangerouslySetInnerHTML={{
          __html: mod.descriptionHtml || "<p>Este mod no tiene descripción.</p>",
        }}
      />
      <div className="mt-4 flex flex-wrap gap-2">
        {mod.genres.map((genre) => (
          <span
            key={`${mod.slug}-${genre}`}
            className="rounded-full border border-slate-700 bg-slate-950 px-3 py-1 text-xs text-slate-300"
          >
            {genre}
          </span>
        ))}
        {mod.genres.length === 0 && (
          <span className="text-xs text-slate-500">Sin géneros registrados.</span>
        )}
      </div>
    </div>
  );
}

function CreditsSection({ mod }: { mod: SupportedMod }) {
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900 p-5">
      <h3 className="text-sm font-semibold text-slate-200">Créditos</h3>
      <div className="mt-4 grid gap-4 md:grid-cols-3">
        {[
          { title: "Creadores", data: mod.credits.creators },
          { title: "Traductores", data: mod.credits.translators },
          { title: "Port", data: mod.credits.porters },
        ].map((group) => (
          <div
            key={`${mod.slug}-${group.title}`}
            className="rounded-lg border border-slate-800 bg-slate-950 p-3"
          >
            <h4 className="text-xs font-semibold uppercase tracking-wide text-slate-400">
              {group.title}
            </h4>
            <ul className="mt-2 space-y-1 text-sm text-slate-200">
              {group.data.length > 0 ? (
                group.data.map((name) => <li key={name}>{name}</li>)
              ) : (
                <li className="text-slate-500">Sin datos</li>
              )}
            </ul>
          </div>
        ))}
      </div>
    </div>
  );
}

function ScreenshotsSection({ mod }: { mod: SupportedMod }) {
  const [activeIdx, setActiveIdx] = useState<number | null>(null);

  const handlePrev = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (activeIdx !== null) {
      setActiveIdx((activeIdx - 1 + mod.screenshotUrls.length) % mod.screenshotUrls.length);
    }
  };

  const handleNext = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (activeIdx !== null) {
      setActiveIdx((activeIdx + 1) % mod.screenshotUrls.length);
    }
  };

  useEffect(() => {
    if (activeIdx === null) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setActiveIdx(null);
      } else if (e.key === "ArrowLeft") {
        setActiveIdx((prev) => (prev !== null ? (prev - 1 + mod.screenshotUrls.length) % mod.screenshotUrls.length : null));
      } else if (e.key === "ArrowRight") {
        setActiveIdx((prev) => (prev !== null ? (prev + 1) % mod.screenshotUrls.length : null));
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [activeIdx, mod.screenshotUrls.length]);

  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900 p-5">
      <h3 className="text-sm font-semibold text-slate-200">{strings.screenshots}</h3>
      {mod.screenshotUrls.length > 0 ? (
        <div className="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {mod.screenshotUrls.map((url, idx) => (
            <div
              key={url}
              onClick={() => setActiveIdx(idx)}
              className="group relative cursor-pointer overflow-hidden rounded-lg border border-slate-800 bg-slate-950 aspect-video shadow-md hover:shadow-xl transition-all duration-300 hover:border-yellow-500/50"
            >
              <img
                src={url}
                alt={`Captura de ${mod.name}`}
                className="h-full w-full object-cover transition-all duration-300 group-hover:scale-105"
              />
              <div className="absolute inset-0 bg-slate-950/0 group-hover:bg-slate-950/40 transition-all duration-300 flex items-center justify-center">
                <span className="opacity-0 group-hover:opacity-100 transition-opacity duration-300 bg-slate-900/90 backdrop-blur-sm border border-slate-700/50 text-white text-xs font-semibold px-3 py-1.5 rounded-full shadow-lg">
                  Ver
                </span>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <p className="mt-3 text-sm text-slate-500">
          {strings.noScreenshots}
        </p>
      )}

      {activeIdx !== null && (
        <div
          onClick={() => setActiveIdx(null)}
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/90 backdrop-blur-sm p-4 md:p-8 select-none"
        >
          <button
            type="button"
            onClick={() => setActiveIdx(null)}
            className="absolute top-4 right-4 text-slate-400 hover:text-white p-2 transition rounded-full hover:bg-slate-800/50 z-50"
            title={strings.close}
          >
            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2} stroke="currentColor" className="w-6 h-6">
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>

          {mod.screenshotUrls.length > 1 && (
            <button
              type="button"
              onClick={handlePrev}
              className="absolute left-4 md:left-8 top-1/2 -translate-y-1/2 text-slate-300 hover:text-white bg-slate-900/60 hover:bg-slate-850 p-3 md:p-4 rounded-full transition shadow-xl border border-slate-850 hover:scale-105 z-50 focus:outline-none"
              title={strings.previous}
            >
              <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2.5} stroke="currentColor" className="w-6 h-6">
                <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 19.5L8.25 12l7.5-7.5" />
              </svg>
            </button>
          )}

          <div className="relative max-h-[85vh] max-w-[85vw] flex items-center justify-center">
            <img
              src={mod.screenshotUrls[activeIdx]}
              alt={`Ampliado: ${mod.name}`}
              onClick={(e) => e.stopPropagation()}
              className="max-h-[85vh] max-w-[85vw] object-contain rounded-lg border border-slate-800 shadow-2xl select-text"
            />
          </div>

          {mod.screenshotUrls.length > 1 && (
            <button
              type="button"
              onClick={handleNext}
              className="absolute right-4 md:right-8 top-1/2 -translate-y-1/2 text-slate-300 hover:text-white bg-slate-900/60 hover:bg-slate-850 p-3 md:p-4 rounded-full transition shadow-xl border border-slate-850 hover:scale-105 z-50 focus:outline-none"
              title={strings.next}
            >
              <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={2.5} stroke="currentColor" className="w-6 h-6">
                <path strokeLinecap="round" strokeLinejoin="round" d="M8.25 4.5l7.5 7.5-7.5 7.5" />
              </svg>
            </button>
          )}

          <div className="absolute bottom-4 left-1/2 -translate-x-1/2 bg-slate-900/80 px-4 py-1.5 rounded-full text-xs text-slate-300 border border-slate-800 font-medium">
            {activeIdx + 1} / {mod.screenshotUrls.length}
          </div>
        </div>
      )}
    </div>
  );
}

function formatBytes(bytes?: number | null): string {
  if (bytes === undefined || bytes === null || bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatSpeed(bytesPerSec?: number | null): string {
  if (!bytesPerSec) return "";
  return `${formatBytes(bytesPerSec)}/s`;
}

function formatEta(seconds?: number | null): string {
  if (seconds === undefined || seconds === null || seconds === 0) return "";
  if (seconds < 60) return `${seconds}s`;
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  return `${mins}m ${secs}s`;
}

function GlobalProgressFooter({
  tasks,
  statusMessage,
  onCancel,
}: {
  tasks: UiInstallationTask[];
  statusMessage: string;
  onCancel: (slug: string) => void;
}) {
  const [showStatus, setShowStatus] = useState(false);

  return (
    <footer className="border-t border-slate-800 bg-slate-900 px-4 py-3">
      <div className="grid gap-2 md:grid-cols-[1fr_auto] items-center">
        <div className="min-h-[58px] space-y-2">
          {tasks.length === 0 ? (
            <p className="text-xs text-slate-400">
            </p>
          ) : (
            tasks.slice(0, 3).map((task) => {
              const isDownloading = task.speed !== undefined && task.speed !== null && task.speed > 0;
              const isRunningOrQueued = task.state === "running" || task.state === "queued";
              const showCancel = isRunningOrQueued;

              return (
                <div
                  key={task.slug}
                  className="rounded-md border border-slate-800 bg-slate-950 px-3 py-2 animate-fade-in"
                >
                  <div className="mb-1 flex items-center justify-between text-xs">
                    <span className="font-medium text-slate-200">
                      {task.slug}
                      {isDownloading && (
                        <span className="text-[10px] text-slate-400 ml-2 font-normal">
                          ({formatBytes(task.downloaded)} / {formatBytes(task.total)} - {formatSpeed(task.speed)} - {formatEta(task.eta)} {strings.eta})
                        </span>
                      )}
                    </span>
                    <div className="flex items-center gap-2">
                      <span
                        className={
                          task.state === "failed"
                            ? "text-rose-400 font-semibold"
                            : task.state === "cancelled"
                              ? "text-amber-400 font-semibold"
                              : task.state === "success"
                                ? "text-emerald-400 font-semibold"
                                : "text-slate-400"
                        }
                      >
                        {task.state === "queued" ? strings.queued : task.state === "failed" ? strings.installFailed : task.state === "cancelled" ? strings.installCancelled : task.state === "success" ? strings.installSuccess : task.state}
                      </span>
                      {showCancel && (
                        <button
                          type="button"
                          onClick={() => onCancel(task.slug)}
                          className="text-[11px] text-rose-400 hover:text-rose-300 transition underline focus:outline-none ml-1.5"
                          title={strings.cancel}
                        >
                          {strings.cancel}
                        </button>
                      )}
                    </div>
                  </div>
                  <p className="truncate text-xs text-slate-400">
                    {task.status === "Downloading mod..." ? strings.preparingMod : task.status}
                  </p>
                  <div className="mt-1 h-1.5 overflow-hidden rounded bg-slate-800">
                    <div
                      className="h-full bg-gradient-to-r from-yellow-500 to-orange-500 transition-all duration-300"
                      style={{ width: `${Math.max(0, Math.min(100, task.progress))}%` }}
                    />
                  </div>
                </div>
              );
            })
          )}
        </div>
        <div className="flex flex-col items-end gap-1.5 justify-center self-end">
          {showStatus ? (
            <div className="flex flex-col gap-1 w-full md:w-[320px]">
              <div className="flex items-center justify-between text-[10px] text-slate-400 font-semibold uppercase tracking-wider px-0.5">
                <span>{strings.notifications}</span>
                <button
                  type="button"
                  onClick={() => setShowStatus(false)}
                  className="hover:text-white transition underline normal-case font-normal text-slate-300 hover:no-underline"
                >
                  {strings.hide}
                </button>
              </div>
              <div className="rounded-md border border-slate-800 bg-slate-950 px-3 py-2 text-xs text-slate-300 animate-fade-in shadow-xl">
                {statusMessage || "Sin notificaciones recientes."}
              </div>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => setShowStatus(true)}
              className="rounded-md border border-slate-800 bg-slate-950 hover:bg-slate-800/50 hover:border-slate-700 px-3 py-2 text-xs text-slate-300 transition flex items-center gap-1.5 focus:outline-none shrink-0 shadow-md font-medium"
            >
              <span>{strings.showNotifications}</span>
            </button>
          )}
        </div>
      </div>
    </footer>
  );
}

function topNavigationLinkClassName({
  isActive,
}: {
  isActive: boolean;
}) {
  return `rounded-md border px-3 py-1.5 text-sm transition ${isActive
    ? "border-yellow-500/50 bg-slate-800 text-yellow-300"
    : "border-slate-800 bg-slate-950 text-slate-200 hover:border-slate-700 hover:bg-slate-800"
    }`;
}

function statusBadgeClasses(status: string) {
  const normalized = status.trim().toLowerCase();
  if (normalized === "stable") {
    return "border-emerald-500/30 bg-emerald-500/15 text-emerald-300";
  }
  if (normalized === "beta") {
    return "border-amber-500/30 bg-amber-500/15 text-amber-300";
  }
  if (normalized === "abandoned") {
    return "border-rose-500/30 bg-rose-500/15 text-rose-300";
  }
  return "border-slate-500/30 bg-slate-500/15 text-slate-300";
}

function fallbackMod(installed: InstalledMod): SupportedMod {
  return {
    slug: installed.slug,
    name: installed.slug,
    downloadUrl: null,
    downloadable: false,
    status: "Instalado",
    currentVersion: installed.currentVersion,
    executable: "",
    descriptionHtml:
      "<p>Instalación local detectada. No hay datos remotos cargados para este mod.</p>",
    heroImageUrl: null,
    logoImageUrl: null,
    screenshotUrls: [],
    genres: [],
    credits: {
      creators: [],
      translators: [],
      porters: [],
    },
  };
}

export default App;
