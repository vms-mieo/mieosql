// Engine glyphs — real brand logos (simple-icons paths, see ./engine-logos).

import type { Engine } from "../lib/bindings";
import { ENGINE_LOGOS } from "../lib/engine-logos";

export function EngineGlyph({
  engine,
  size = 14,
  muted = false,
}: {
  engine: Engine;
  /** Logo size in px (the box is slightly larger for alignment). */
  size?: number;
  /** Disconnected profiles: neutral gray instead of the brand colour. */
  muted?: boolean;
}) {
  const logo = ENGINE_LOGOS[engine];
  if (!logo) return null;
  const color = muted ? "#b4b4bb" : logo.color;
  return (
    <span
      className="grid shrink-0 place-items-center"
      style={{ width: size + 5, height: size + 5 }}
    >
      <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
        <path d={logo.path} fill={color} />
      </svg>
    </span>
  );
}
