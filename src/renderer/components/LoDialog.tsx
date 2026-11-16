import type { ReactNode } from "react";
import { Modal } from "reshaped";

/**
 * A centred dialog in the Merged look (rounded card, hairline, soft shadow). Reshaped's Modal supplies
 * focus handling, Esc and the scrim; everything inside is the lo kit (LoField, LoButton).
 */
export function LoDialog({
  open,
  onClose,
  title,
  description,
  width = 520,
  children,
  footer,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  width?: number;
  children: ReactNode;
  footer: ReactNode;
}) {
  return (
    <Modal
      active={open}
      onClose={onClose}
      size={`${width}px`}
      padding={0}
      className="lo-dialog-modal"
    >
      <div className="lo-dialog">
        <h2>{title}</h2>
        {description && <div className="lo-sub">{description}</div>}
        <div className="lo-dialog-body">{children}</div>
        <div className="lo-dialog-foot">{footer}</div>
      </div>
    </Modal>
  );
}
