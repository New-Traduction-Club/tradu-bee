export interface InstalledMod {
  slug: string;
  installPath: string;
  currentVersion: string | null;
  executablePath: string;
  installedAtEpochMs: number;
}

export interface LauncherStateView {
  manifestUrl: string | null;
  globalInstallDir: string;
  cachedDdlcZipPath: string | null;
  oobeCompleted: boolean;
  installedMods: InstalledMod[];
  expectedDdlcSha256: string;
  manifestUrlHint: string;
}

export interface UpdateLauncherConfigRequest {
  manifestUrl?: string | null;
  globalInstallDir?: string | null;
  cachedDdlcZipPath?: string | null;
}

export interface SupportedModCredits {
  creators: string[];
  translators: string[];
  porters: string[];
}

export interface SupportedMod {
  slug: string;
  name: string;
  downloadUrl: string | null;
  downloadable: boolean;
  status: string;
  currentVersion: string | null;
  executable: string;
  descriptionHtml: string;
  heroImageUrl: string | null;
  logoImageUrl: string | null;
  screenshotUrls: string[];
  genres: string[];
  credits: SupportedModCredits;
}

export interface InstallationEvent {
  slug: string;
  status: string;
  message: string;
}

export interface InstallationProgressEvent {
  slug: string;
  progress: number;
  status: string;
  state: "queued" | "running" | "success" | "failed" | string;
  error?: string | null;
}

export interface ModProcessStatusEvent {
  slug: string;
  isRunning: boolean;
  pid?: number | null;
}
