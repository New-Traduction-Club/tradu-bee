import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
      setStatusMessage(
        "Este mod requiere flujo manual: abre la descarga y selecciona un .zip/.rar.",
      );
      return;
    }

    setInstallRequestSlug(mod.slug);
    try {
      await invoke("execute_installation_recipe", {
        slug: mod.slug,
        userProvidedZipPath: mod.downloadable ? null : manualArchivePath,
      });
      setStatusMessage(`Instalación en segundo plano iniciada para ${mod.name}.`);
    } catch (error) {
      setStatusMessage(String(error));
    } finally {
      setInstallRequestSlug(null);
    }
  }

  async function uninstallInstalledMod(slug: string) {
    setUninstallingSlug(slug);
    setStatusMessage(`Desinstalando ${slug}...`);
    try {
      await invoke("uninstall_mod", { slug });
      await refreshLauncherState();
      setStatusMessage(`Desinstalación completada: ${slug}`);
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

          if (payload.state === "success" || payload.state === "failed") {
            const slug = payload.slug;
            window.setTimeout(() => {
              setTasksBySlug((current) => {
                if (!current[slug] || current[slug].state === "running") {
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
      }, 2500);
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

      <GlobalProgressFooter tasks={progressTasks} statusMessage={statusMessage} />
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
                  className={`w-full rounded-lg border p-2 text-left transition ${
                    selected
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
                <h3 className="text-sm font-semibold text-slate-200">Acciones</h3>
                <div className="mt-3 flex flex-wrap items-center gap-3">
                  {selectedInstalled ? (
                    <>
                      <button
                        type="button"
                        onClick={() => void onPlay(selectedMod.slug)}
                        className={`rounded-lg px-5 py-2.5 text-sm font-semibold transition ${
                          selectedIsRunning
                            ? "cursor-not-allowed border border-emerald-400/40 bg-emerald-500/10 text-emerald-300"
                            : "bg-gradient-to-r from-yellow-500 to-orange-500 text-slate-950 hover:brightness-110"
                        }`}
                        disabled={selectedIsRunning}
                      >
                        {selectedIsRunning ? "Ejecutando" : "Jugar"}
                      </button>
                      <button
                        type="button"
                        onClick={() => void onUninstall(selectedMod.slug)}
                        className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2.5 text-sm font-medium text-slate-100 transition hover:border-slate-600 disabled:cursor-not-allowed disabled:opacity-60"
                        disabled={uninstallingSlug === selectedMod.slug}
                      >
                        {uninstallingSlug === selectedMod.slug
                          ? "Desinstalando..."
                          : "Desinstalar"}
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
                        ? "Procesando..."
                        : "Instalar"}
                    </button>
                  )}

                  {!selectedInstalled && !selectedMod.downloadable && (
                    <>
                      <button
                        type="button"
                        onClick={() => void onOpenManualDownload(selectedMod)}
                        className="rounded-lg border border-amber-500/50 bg-amber-500/10 px-4 py-2.5 text-sm font-medium text-amber-200 transition hover:border-amber-400"
                      >
                        Descarga manual
                      </button>
                      <button
                        type="button"
                        onClick={() => void onSelectManualArchive(selectedMod.slug)}
                        className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2.5 text-sm font-medium text-slate-100 transition hover:border-slate-600"
                      >
                        Seleccionar .zip/.rar
                      </button>
                    </>
                  )}
                </div>

                {!selectedInstalled && !selectedMod.downloadable && (
                  <p className="mt-3 text-xs text-amber-300">
                    Este mod requiere descarga manual.
                    {manualArchiveBySlug[selectedMod.slug]
                      ? ` Archivo seleccionado: ${manualArchiveBySlug[selectedMod.slug]}`
                      : " Después de descargar, selecciona el archivo para instalar."}
                  </p>
                )}
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
              const isRunning = runningProcessSlugs.has(mod.slug);
              return (
                <button
                  key={mod.slug}
                  type="button"
                  onClick={() => onSelect(mod.slug)}
                  className={`w-full rounded-lg border p-2 text-left transition ${
                    isSelected
                      ? "border-yellow-500/60 bg-slate-800"
                      : "border-slate-800 bg-slate-950 hover:bg-slate-800/50"
                  }`}
                >
                  <p className="truncate text-sm font-medium">{mod.name}</p>
                  <p className="mt-1 truncate text-xs text-slate-400">
                    {isRunning ? "Ejecutando" : "Listo para jugar"}
                  </p>
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
            Instala un mod desde Explorar mods para verlo en biblioteca.
          </div>
        ) : (
          <div className="pb-8">
            <ModHeroSection mod={selected} />
            <section className="-mt-7 px-6">
              <div className="rounded-xl border border-slate-800 bg-slate-900 p-4 shadow-2xl">
                <h3 className="text-sm font-semibold text-slate-200">Acciones</h3>
                <div className="mt-3 flex gap-3">
                  <button
                    type="button"
                    onClick={() => void onPlay(selected.slug)}
                    className={`rounded-lg px-5 py-2.5 text-sm font-semibold transition ${
                      selectedIsRunning
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
    <section className="relative h-72 border-b border-slate-800">
      {mod.heroImageUrl ? (
        <img
          src={mod.heroImageUrl}
          alt={`${mod.name} portada`}
          className="absolute inset-0 h-full w-full object-cover"
        />
      ) : (
        <div className="absolute inset-0 bg-slate-900" />
      )}
      <div className="absolute inset-0 bg-gradient-to-t from-slate-950 via-slate-900/50 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 p-6">
        <div className="flex items-end gap-4">
          {mod.logoImageUrl && (
            <img
              src={mod.logoImageUrl}
              alt={`${mod.name} logo`}
              className="h-20 w-20 rounded-xl border border-slate-700 bg-slate-900 object-cover"
            />
          )}
          <div>
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
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900 p-5">
      <h3 className="text-sm font-semibold text-slate-200">Capturas</h3>
      {mod.screenshotUrls.length > 0 ? (
        <div className="mt-4 grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
          {mod.screenshotUrls.map((url) => (
            <div
              key={url}
              className="overflow-hidden rounded-lg border border-slate-800 bg-slate-950"
            >
              <img
                src={url}
                alt={`Captura de ${mod.name}`}
                className="h-40 w-full object-cover"
              />
            </div>
          ))}
        </div>
      ) : (
        <p className="mt-3 text-sm text-slate-500">
          Este mod no tiene capturas.
        </p>
      )}
    </div>
  );
}

function GlobalProgressFooter({
  tasks,
  statusMessage,
}: {
  tasks: UiInstallationTask[];
  statusMessage: string;
}) {
  return (
    <footer className="border-t border-slate-800 bg-slate-900 px-4 py-3">
      <div className="grid gap-2 md:grid-cols-[1fr_320px]">
        <div className="min-h-[58px] space-y-2">
          {tasks.length === 0 ? (
            <p className="text-xs text-slate-400">
              Gestor de descargas: sin tareas activas.
            </p>
          ) : (
            tasks.slice(0, 3).map((task) => (
              <div
                key={task.slug}
                className="rounded-md border border-slate-800 bg-slate-950 px-3 py-2"
              >
                <div className="mb-1 flex items-center justify-between text-xs">
                  <span className="font-medium text-slate-200">{task.slug}</span>
                  <span
                    className={
                      task.state === "failed"
                        ? "text-rose-300"
                        : task.state === "success"
                          ? "text-emerald-300"
                          : "text-slate-400"
                    }
                  >
                    {task.state}
                  </span>
                </div>
                <p className="truncate text-xs text-slate-400">{task.status}</p>
                <div className="mt-1 h-1.5 overflow-hidden rounded bg-slate-800">
                  <div
                    className="h-full bg-gradient-to-r from-yellow-500 to-orange-500 transition-all"
                    style={{ width: `${Math.max(0, Math.min(100, task.progress))}%` }}
                  />
                </div>
              </div>
            ))
          )}
        </div>
        <div className="rounded-md border border-slate-800 bg-slate-950 px-3 py-2 text-xs text-slate-300">
          {statusMessage || "Sin notificaciones recientes."}
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
  return `rounded-md border px-3 py-1.5 text-sm transition ${
    isActive
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
