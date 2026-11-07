import { polar, sectorPath } from '../shared/geometry';
import type { Geom, WheelView } from '../shared/types';
import { Icon } from './Icon';

/** A static, scaled-down drawing of a wheel, using the same geometry as the overlay. */
export function WheelPreview({ wheel, geom, appIcons, size = 220 }: { wheel: WheelView; geom: Geom; appIcons: Record<string, string>; size?: number }) {
  const n = wheel.items.length;
  const scale = size / geom.win;
  const c = geom.win / 2;
  return (
    <div className="preview" style={{ width: size, height: size }}>
      <svg viewBox={`0 0 ${geom.win} ${geom.win}`} width={size} height={size}>
        {wheel.items.map((it, i) => (
          <path key={it.id} d={sectorPath(i, n, geom, 24)} className={'pv-sector' + (it.submenu ? ' pv-sub' : '')} />
        ))}
        <circle cx={c} cy={c} r={geom.r_center} className="pv-disc" />
      </svg>
      {wheel.items.map((it, i) => {
        const [x, y] = polar(i, n, geom);
        return (
          <div key={it.id} className="pv-item" style={{ left: x * scale, top: y * scale }} title={it.label}>
            <Icon name={it.icon} size={Math.round(18 * Math.max(scale, 0.7))} appIcons={appIcons} />
          </div>
        );
      })}
      <div className="pv-name">{wheel.name}</div>
    </div>
  );
}
