import { FormEvent, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { LauncherStateView } from "../types/launcher";

interface OobeSetupViewProps {
  expectedHash: string;
  initialOriginalZipPath?: string | null;
  initialInstallDir?: string;
  onCompleted: (state: LauncherStateView) => void;
  onStatus: (message: string) => void;
}

function OobeSetupView({
  initialOriginalZipPath,
  initialInstallDir,
  onCompleted,
  onStatus,
}: OobeSetupViewProps) {
  const [originalZipPath, setOriginalZipPath] = useState(
    initialOriginalZipPath ?? "",
  );
  const [installDir, setInstallDir] = useState(initialInstallDir ?? "");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function selectOriginalZip() {
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "ZIP original DDLC", extensions: ["zip"] }],
      });
      if (typeof selected === "string") {
        setOriginalZipPath(selected);
        setError(null);
      }
    } catch (selectionError) {
      setError(String(selectionError));
    }
  }

  async function selectInstallDir() {
    try {
      const selected = await openDialog({
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        setInstallDir(selected);
      }
    } catch (selectionError) {
      setError(String(selectionError));
    }
  }

  async function finalizeSetup(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!originalZipPath.trim()) {
      setError("Selecciona primero el ZIP original de DDLC.");
      return;
    }

    setSubmitting(true);
    setError(null);
    onStatus("Validando archivo original y aplicando configuración inicial...");
    try {
      const state = await invoke<LauncherStateView>("finalize_oobe_setup", {
        originalZipPath,
        globalInstallDir: installDir.trim() || null,
      });
      onCompleted(state);
      onStatus("Configuración inicial completada correctamente.");
    } catch (setupError) {
      setError(String(setupError));
      onStatus(String(setupError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center bg-[#0f172a] p-8">
      <div className="w-full max-w-3xl rounded-2xl border border-slate-800 bg-slate-900/90 p-8 shadow-2xl">
        <div className="mb-6 space-y-2">
          <p className="text-xs font-semibold uppercase tracking-[0.2em] text-amber-300">
            Configuración inicial
          </p>
          <h1 className="text-3xl font-semibold tracking-tight text-white">
            Bienvenido a Tradu-Bee Launcher
          </h1>
          <p className="text-sm text-slate-300">
            Selecciona el ZIP original de DDLC para habilitar la instalación de
            mods. También puedes definir tu carpeta de instalaciones.
          </p>
        </div>

        <form className="space-y-4" onSubmit={finalizeSetup}>
          <label className="block text-sm text-slate-200">
            ZIP original de DDLC
            <input
              className="mt-2 w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 outline-none focus:border-yellow-500"
              value={originalZipPath}
              onChange={(event) => setOriginalZipPath(event.currentTarget.value)}
              placeholder="C:\\ruta\\ddlc-win.zip"
            />
          </label>

          <label className="block text-sm text-slate-200">
            Carpeta de instalaciones (opcional)
            <input
              className="mt-2 w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 outline-none focus:border-yellow-500"
              value={installDir}
              onChange={(event) => setInstallDir(event.currentTarget.value)}
              placeholder="C:\\Users\\<usuario>\\AppData\\Local\\TraduBee\\Mods"
            />
          </label>

          {/* <div className="rounded-lg border border-slate-800 bg-slate-950 p-3 text-xs text-slate-300">
            Hash esperado:
            <span className="ml-2 break-all font-mono text-amber-200">
              {expectedHash}
            </span>
          </div> */}

          <div className="flex flex-wrap gap-3">
            <button
              type="button"
              onClick={selectOriginalZip}
              className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-sm font-medium text-slate-100 transition hover:border-slate-500 disabled:cursor-not-allowed disabled:opacity-60"
              disabled={submitting}
            >
              Seleccionar ZIP
            </button>
            <button
              type="button"
              onClick={selectInstallDir}
              className="rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-sm font-medium text-slate-100 transition hover:border-slate-500 disabled:cursor-not-allowed disabled:opacity-60"
              disabled={submitting}
            >
              Seleccionar carpeta
            </button>
            <button
              type="submit"
              className="rounded-lg bg-gradient-to-r from-yellow-500 to-orange-500 px-5 py-2 text-sm font-semibold text-slate-950 transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
              disabled={submitting}
            >
              {submitting ? "Aplicando..." : "Finalizar configuración"}
            </button>
          </div>
        </form>

        {error && (
          <p className="mt-4 rounded-lg border border-rose-500/40 bg-rose-500/10 px-3 py-2 text-sm text-rose-200">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}

export default OobeSetupView;
