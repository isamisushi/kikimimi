import { useEffect, useRef, type ComponentProps, type ReactNode } from "react";
import { cn } from "./utils";
export function NativeSelect({
  className,
  ...props
}: ComponentProps<"select">) {
  return (
    <select
      data-slot="select"
      className={cn("ui-select", className)}
      {...props}
    />
  );
}
export function Card({ className, ...props }: ComponentProps<"section">) {
  return (
    <section data-slot="card" className={cn("ui-card", className)} {...props} />
  );
}
/** Native modal supplies focus trapping, Escape, and background inertness. */
export function Modal({
  open,
  onClose,
  children,
  ...props
}: Omit<ComponentProps<"dialog">, "open" | "onClose"> & {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (open && !dialog?.open) dialog?.showModal();
    else if (!open && dialog?.open) dialog.close();
  }, [open]);
  return (
    <dialog
      {...props}
      ref={ref}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClose={onClose}
    >
      {children}
    </dialog>
  );
}
