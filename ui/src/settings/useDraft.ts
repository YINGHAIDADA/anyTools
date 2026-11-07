import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { clone, type ConfigFile } from './model';

export interface Draft {
  cfg: ConfigFile | null;
  dirty: boolean;
  /** Error from the Rust validator for the current draft (null = valid). */
  error: string | null;
  validating: boolean;
  saving: boolean;
  edit: (f: (c: ConfigFile) => void) => void;
  save: () => Promise<boolean>;
  discard: () => void;
  /** Reloads from disk (after an external edit), dropping the draft. */
  reset: () => Promise<void>;
}

/**
 * An editable copy of the config. Every change is validated by the same Rust code that loads
 * config.yaml (debounced), so the editor can never save something the app would reject.
 */
export function useDraft(): Draft {
  const [saved, setSaved] = useState<ConfigFile | null>(null);
  const [cfg, setCfg] = useState<ConfigFile | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [validating, setValidating] = useState(false);
  const [saving, setSaving] = useState(false);
  const seq = useRef(0);

  const reset = useCallback(async () => {
    const c = await invoke<ConfigFile>('get_config');
    setSaved(c);
    setCfg(clone(c));
    setError(null);
  }, []);

  useEffect(() => {
    void reset();
  }, [reset]);

  const dirty = cfg !== null && saved !== null && JSON.stringify(cfg) !== JSON.stringify(saved);

  useEffect(() => {
    if (!cfg || !dirty) {
      setError(null);
      return;
    }
    const my = ++seq.current;
    setValidating(true);
    const t = setTimeout(async () => {
      try {
        await invoke('validate_config', { cfg });
        if (my === seq.current) setError(null);
      } catch (e) {
        if (my === seq.current) setError(String(e));
      } finally {
        if (my === seq.current) setValidating(false);
      }
    }, 250);
    return () => clearTimeout(t);
  }, [cfg, dirty]);

  const edit = useCallback((f: (c: ConfigFile) => void) => {
    setCfg((c) => {
      if (!c) return c;
      const next = clone(c);
      f(next);
      return next;
    });
  }, []);

  const save = useCallback(async () => {
    if (!cfg) return false;
    setSaving(true);
    try {
      await invoke('save_config', { cfg });
      setSaved(clone(cfg));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  }, [cfg]);

  const discard = useCallback(() => {
    if (saved) setCfg(clone(saved));
    setError(null);
  }, [saved]);

  return { cfg, dirty, error, validating, saving, edit, save, discard, reset };
}
