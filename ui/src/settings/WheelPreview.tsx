import { polar, sectorPath } from '../shared/geometry';
import type { Geom, WheelView } from '../shared/types';
import { Icon } from './Icon';

/** A scaled-down drawing of a wheel, using the same geometry as the overlay. Optionally clickable. */
export function WheelPreview(props: {
  wheel: WheelView;
  geom: Geom;
  appIcons: Record<string, string>;
  size?: number;
  selected?: number | null;
  onSelect?: (i: number | null) => void;
}) {
  const { wheel, geom, appIcons, size = 220, selected = null, onSelect } = props;
  const n = wheel.items.length;
  const scale = size / geom.win;
  const c = geom.win / 2;
  return (
    <div className={'preview' + (onSelect ? ' interactive' : '')} style={{ width: size, height: size }}>
      <svg viewBox={`0 0 ${geom.win} ${geom.win}`} width={size} height={size}>
        {wheel.items.map((it, i) => (
          <path
            key={it.id + i}
            d={sectorPath(i, n, geom, 24)}
            className={'pv-sector' + (it.submenu ? ' pv-sub' : '') + (selected === i ? ' pv-sel' : '')}
            onClick={onSelect ? () => onSelect(i) : undefined}
          />
        ))}
        <circle cx={c} cy={c} r={geom.r_center} className="pv-disc" onClick={onSelect ? () => onSelect(null) : undefined} />
      </svg>
      {wheel.items.map((it, i) => {
        const [x, y] = polar(i, n, geom);
        return (
          <div key={it.id + i} className="pv-item" style={{ left: x * scale, top: y * scale }} title={it.label}>
            <Icon name={it.icon} size={Math.round(18 * Math.max(scale, 0.7))} appIcons={appIcons} />
          </div>
        );
      })}
      <div className="pv-name">{selected !== null && wheel.items[selected] ? wheel.items[selected].label : wheel.name}</div>
    </div>
  );
}
