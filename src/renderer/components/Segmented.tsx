export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  ariaLabel?: string;
}

/** Pressed-button segmented control (year switcher, filters). Arrow keys move between options. */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  label,
  fill,
}: {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (v: T) => void;
  label: string;
  /** Stretch to the full width with equal-sized options (for a control that leads a dialog). */
  fill?: boolean;
}) {
  return (
    <div
      className={fill ? "lo-seg lo-seg--fill" : "lo-seg"}
      role="group"
      aria-label={label}
      onKeyDown={(e) => {
        if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
        const i = options.findIndex((o) => o.value === value);
        const next =
          options[
            (i + (e.key === "ArrowRight" ? 1 : -1) + options.length) %
              options.length
          ];
        onChange(next.value);
        const btns = e.currentTarget.querySelectorAll("button");
        (btns[options.indexOf(next)] as HTMLButtonElement | undefined)?.focus();
        e.preventDefault();
      }}
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          aria-pressed={o.value === value}
          aria-label={o.ariaLabel}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
