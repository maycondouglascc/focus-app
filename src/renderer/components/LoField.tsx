import type {
  InputHTMLAttributes,
  ReactNode,
  TextareaHTMLAttributes,
} from "react";
import { useId } from "react";

/** A labelled form control. The control gets the label's id via the render prop. */
export function LoField({
  label,
  required,
  children,
}: {
  label: string;
  required?: boolean;
  children: (id: string) => ReactNode;
}) {
  const id = useId();
  return (
    <div className="lo-field">
      <label className="lo-label" htmlFor={id}>
        {label}
        {required && <i aria-hidden="true"> *</i>}
      </label>
      {children(id)}
    </div>
  );
}

export const LoInput = (p: InputHTMLAttributes<HTMLInputElement>) => (
  <input spellCheck={false} {...p} className={`lo-input ${p.className ?? ""}`} />
);

export const LoTextarea = (p: TextareaHTMLAttributes<HTMLTextAreaElement>) => (
  <textarea {...p} className={`lo-textarea ${p.className ?? ""}`} />
);

export { LoSelect } from "./LoSelect";
