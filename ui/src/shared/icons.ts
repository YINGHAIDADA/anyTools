import { createElement, icons, type IconNode } from 'lucide';

const SVGNS = 'http://www.w3.org/2000/svg';
const lucide = icons as Record<string, IconNode>;

export function hasLucide(name: string): boolean {
  return name in lucide;
}

/** Renders `lucide:Name` (falls back to a circle). App icons are handled by the caller. */
export function lucideSvg(name: string, size: number, strokeWidth = 1.8): SVGElement {
  return createElement(lucide[name] ?? lucide.Circle, { width: size, height: size, 'stroke-width': strokeWidth });
}

/**
 * An icon as an SVG element of the given size: lucide glyph, or an app icon from `appIcons`
 * (data URLs keyed by the full `app:<target>` string), or a generic app glyph until it arrives.
 */
export function iconElement(icon: string, size: number, appIcons: Record<string, string>): SVGElement {
  if (icon.startsWith('app:')) {
    const url = appIcons[icon];
    if (url) {
      const svg = document.createElementNS(SVGNS, 'svg');
      svg.setAttribute('width', String(size));
      svg.setAttribute('height', String(size));
      svg.setAttribute('viewBox', `0 0 ${size} ${size}`);
      const img = document.createElementNS(SVGNS, 'image');
      img.setAttribute('href', url);
      img.setAttribute('width', String(size));
      img.setAttribute('height', String(size));
      svg.appendChild(img);
      svg.classList.add('app-icon');
      return svg;
    }
    return lucideSvg('AppWindow', size);
  }
  const name = icon.startsWith('lucide:') ? icon.slice(7) : icon;
  return lucideSvg(name, size);
}
