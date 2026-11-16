/** A project's colour mark: a small macOS-style squircle, the same shape everywhere a project appears. */
export function ProjectDot({ color, size = 8 }: { color: string; size?: number }) {
  return <span aria-hidden="true" className="fo-pdot" style={{ width: size, height: size, background: color }} />;
}
