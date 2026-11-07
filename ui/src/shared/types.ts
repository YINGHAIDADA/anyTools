// Shapes of the data the Rust side sends. Keep in sync with crates/modules/wheel/src/lib.rs (wheel_payload)
// and app/src/host.rs (status).

export interface Geom {
  win: number;
  r_out: number;
  r_in: number;
  r_center: number;
  gap: number;
  dead: number;
}

export interface ItemView {
  id: string;
  label: string;
  /** `lucide:Name`, `app:<target>` or `file:<path>` */
  icon: string;
  submenu: boolean;
}

export interface WheelView {
  key: string;
  name: string;
  items: ItemView[];
}

export interface Snapshot {
  geom: Geom;
  fallback: string;
  wheels: WheelView[];
}

export type PermState = 'granted' | 'denied' | 'not_determined' | 'not_needed' | 'unknown';

export interface Settings {
  theme: 'auto' | 'dark' | 'light';
  accent: string;
  material: string;
  terminal?: unknown;
}

export interface Status {
  version: string;
  platform: 'mac' | 'win' | 'other';
  hook: boolean;
  secure_input: boolean;
  permissions: Record<'accessibility' | 'automation_finder', PermState>;
  config_path: string;
  config_error: string | null;
  settings: Settings;
  actions: string[];
  modules: { id: string; name: string; permissions: string[]; status: unknown }[];
}
