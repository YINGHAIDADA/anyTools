import { useLayoutEffect, useRef } from 'react';
import { iconElement } from '../shared/icons';

/** Renders a lucide icon (`Name` or `lucide:Name`) or an app icon (`app:…`, from `appIcons`). */
export function Icon({ name, size = 16, appIcons = {} }: { name: string; size?: number; appIcons?: Record<string, string> }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    ref.current?.replaceChildren(iconElement(name, size, appIcons));
  }, [name, size, appIcons]);
  return <span ref={ref} className="icon" style={{ width: size, height: size }} aria-hidden />;
}
