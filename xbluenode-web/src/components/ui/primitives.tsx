import React, { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  Eye,
  EyeOff,
  X,
} from "lucide-react";
import { translateEnum, useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export function Card({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "rounded-md border border-border bg-card text-card-foreground",
        className
      )}
      {...props}
    />
  );
}

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "outline" | "ghost" | "danger";
  size?: "sm" | "md" | "lg";
}

export function Button({
  variant = "primary",
  size = "md",
  className,
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(
        "inline-flex items-center justify-center gap-1.5 rounded-md font-medium whitespace-nowrap shrink-0 transition-colors disabled:pointer-events-none disabled:opacity-40 cursor-pointer",
        size === "sm" && "h-7 px-2.5 text-xs",
        size === "md" && "h-8.5 px-3.5 py-1.5 text-xs",
        size === "lg" && "h-9 px-4 text-sm",
        variant === "primary" &&
          "bg-primary text-primary-foreground hover:opacity-90",
        variant === "outline" &&
          "border border-border bg-background hover:bg-muted text-foreground",
        variant === "ghost" &&
          "hover:bg-muted text-muted-foreground hover:text-foreground",
        variant === "danger" &&
          "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400 hover:bg-red-500/20",
        className
      )}
      {...props}
    />
  );
}

export interface IconButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  size?: "sm" | "md";
  active?: boolean;
}

export function IconButton({
  size = "md",
  active = false,
  className,
  ...props
}: IconButtonProps) {
  return (
    <button
      type="button"
      className={cn(
        "inline-flex items-center justify-center rounded-md transition-colors shrink-0 disabled:pointer-events-none disabled:opacity-40 cursor-pointer",
        size === "sm" ? "h-7 w-7" : "h-8 w-8",
        active
          ? "bg-xblue-soft text-xblue"
          : "text-muted-foreground hover:text-foreground hover:bg-muted",
        className
      )}
      {...props}
    />
  );
}

export function Badge({
  variant = "neutral",
  className,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & {
  variant?: "neutral" | "xblue" | "outline" | "success" | "warning" | "danger";
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium font-mono-tech whitespace-nowrap shrink-0",
        variant === "neutral" && "bg-muted text-foreground",
        variant === "xblue" && "bg-xblue-soft text-xblue border border-xblue/25",
        variant === "outline" && "border border-border text-muted-foreground",
        variant === "success" &&
          "bg-emerald-500/12 text-emerald-700 dark:text-emerald-400 border border-emerald-500/25",
        variant === "warning" &&
          "bg-amber-500/12 text-amber-700 dark:text-amber-400 border border-amber-500/25",
        variant === "danger" &&
          "bg-red-500/12 text-red-700 dark:text-red-400 border border-red-500/25",
        className
      )}
      {...props}
    />
  );
}

export function StatusBadge({
  status,
  className,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & {
  status: string | null | undefined;
}) {
  const { locale } = useI18n();
  const upper = (status ?? "").toUpperCase();
  let variant: "neutral" | "xblue" | "outline" | "success" | "warning" | "danger" =
    "outline";
  if (
    [
      "ONLINE",
      "OK",
      "READY",
      "HEALTHY",
      "SUCCEEDED",
      "ACTIVE",
      "ENABLED",
      "PUBLISHED",
      "VERIFIED",
      "COMMITTED",
      "DONE",
      "RESOLVED",
      "APPROVED",
      "CONNECTED",
    ].includes(upper)
  ) {
    variant = "success";
  } else if (
    ["RUNNING", "IN_PROGRESS", "LIVE", "ACK", "ACKED"].includes(upper)
  ) {
    variant = "xblue";
  } else if (
    [
      "WARN",
      "WARNING",
      "PENDING",
      "WAITING_APPROVAL",
      "DRAFT",
      "DEGRADED",
      "SUPPRESSED",
    ].includes(upper)
  ) {
    variant = "warning";
  } else if (
    [
      "FAILED",
      "ERROR",
      "CRITICAL",
      "UNHEALTHY",
      "OFFLINE",
      "BLOCKED",
      "LOCKED",
      "REJECTED",
    ].includes(upper)
  ) {
    variant = "danger";
  } else if (status) {
    variant = "neutral";
  }

  return (
    <Badge variant={variant} className={className} {...props}>
      {translateEnum(status, locale)}
    </Badge>
  );
}

export function Input({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-xs placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-xblue disabled:opacity-50",
        className
      )}
      {...props}
    />
  );
}

export function PasswordInput({
  className,
  ...props
}: Omit<React.InputHTMLAttributes<HTMLInputElement>, "type">) {
  const [visible, setVisible] = useState(false);
  const { tx } = useI18n();
  return (
    <div className="relative flex items-center w-full">
      <Input
        {...props}
        type={visible ? "text" : "password"}
        className={cn("pr-9", className)}
      />
      <button
        type="button"
        tabIndex={-1}
        onClick={() => setVisible((v) => !v)}
        aria-label={visible ? tx("隐藏口令", "Hide password") : tx("显示口令", "Show password")}
        title={visible ? tx("隐藏口令", "Hide password") : tx("显示口令", "Show password")}
        className="absolute right-1.5 inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:text-foreground cursor-pointer"
      >
        {visible ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
      </button>
    </div>
  );
}

export function Select({
  className,
  children,
  ...props
}: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={cn(
        "flex h-9 rounded-md border border-input bg-background px-2.5 py-1 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-xblue disabled:opacity-50 cursor-pointer",
        className
      )}
      {...props}
    >
      {children}
    </select>
  );
}

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        "flex w-full rounded-md border border-input bg-background px-3 py-2 text-xs placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-xblue disabled:opacity-50",
        className
      )}
      {...props}
    />
  );
}

export interface PageHeaderProps {
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  badges?: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}

export function PageHeader({
  title,
  subtitle,
  badges,
  actions,
  className,
}: PageHeaderProps) {
  return (
    <div
      className={cn(
        "flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3.5",
        className
      )}
    >
      <div className="min-w-0 space-y-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-base font-semibold tracking-tight">{title}</h1>
          {badges}
        </div>
        {subtitle && (
          <p className="text-xs text-muted-foreground">{subtitle}</p>
        )}
      </div>
      {actions && (
        <div className="flex flex-wrap items-center gap-2 shrink-0">
          {actions}
        </div>
      )}
    </div>
  );
}

export function StatCard({
  label,
  value,
  hint,
  icon,
  className,
}: {
  label: React.ReactNode;
  value: React.ReactNode;
  hint?: React.ReactNode;
  icon?: React.ReactNode;
  className?: string;
}) {
  return (
    <Card className={cn("p-3.5 space-y-1.5", className)}>
      <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="truncate">{label}</span>
        {icon && <span className="shrink-0 text-muted-foreground">{icon}</span>}
      </div>
      <div className="text-xl font-semibold font-mono-tech tracking-tight">
        {value}
      </div>
      {hint && <div className="text-[11px] text-muted-foreground">{hint}</div>}
    </Card>
  );
}

export function EmptyState({
  title,
  action,
  className,
}: {
  title: React.ReactNode;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-md border border-dashed border-border py-10 px-4 text-center space-y-3",
        className
      )}
    >
      <div className="text-xs text-muted-foreground">{title}</div>
      {action && <div className="flex justify-center">{action}</div>}
    </div>
  );
}

export interface TabOption<K extends string = string> {
  readonly key: K;
  readonly label: React.ReactNode;
  readonly icon?: React.ReactNode;
  readonly count?: number;
  readonly disabled?: boolean;
  readonly testId?: string;
}

export function Tabs<K extends string>({
  items,
  value,
  onChange,
  className,
}: {
  items: readonly TabOption<K>[];
  value: K;
  onChange: (next: K) => void;
  className?: string;
}) {
  return (
    <div
      role="tablist"
      className={cn(
        "flex items-center gap-1 border-b border-border overflow-x-auto",
        className
      )}
    >
      {items.map((item) => {
        const active = item.key === value;
        return (
          <button
            key={item.key}
            type="button"
            role="tab"
            aria-selected={active}
            disabled={item.disabled}
            data-testid={item.testId}
            onClick={() => onChange(item.key)}
            className={cn(
              "inline-flex items-center gap-1.5 border-b-2 px-3 py-2 text-xs font-medium whitespace-nowrap shrink-0 transition-colors disabled:opacity-40 disabled:pointer-events-none cursor-pointer -mb-px",
              active
                ? "border-xblue text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground"
            )}
          >
            {item.icon}
            <span>{item.label}</span>
            {typeof item.count === "number" && (
              <span
                className={cn(
                  "rounded px-1.5 py-0.2 text-[10px] font-mono-tech",
                  active
                    ? "bg-xblue-soft text-xblue"
                    : "bg-muted text-muted-foreground"
                )}
              >
                {item.count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

export function SegmentedControl<K extends string>({
  items,
  value,
  onChange,
  className,
}: {
  items: readonly TabOption<K>[];
  value: K;
  onChange: (next: K) => void;
  className?: string;
}) {
  return (
    <div
      role="tablist"
      className={cn(
        "inline-flex items-center gap-0.5 rounded-md border border-border bg-muted/40 p-0.5",
        className
      )}
    >
      {items.map((item) => {
        const active = item.key === value;
        return (
          <button
            key={item.key}
            type="button"
            role="tab"
            aria-selected={active}
            disabled={item.disabled}
            data-testid={item.testId}
            onClick={() => onChange(item.key)}
            className={cn(
              "inline-flex items-center gap-1.5 rounded px-2.5 py-1 text-xs font-medium whitespace-nowrap shrink-0 transition-colors disabled:opacity-40 cursor-pointer",
              active
                ? "bg-background text-foreground shadow-2xs border border-border/80"
                : "text-muted-foreground hover:text-foreground"
            )}
          >
            {item.icon}
            <span>{item.label}</span>
            {typeof item.count === "number" && (
              <span className="text-[10px] font-mono-tech opacity-80">
                {item.count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

export function Pagination({
  page,
  pageSize,
  total,
  onPageChange,
  className,
}: {
  page: number;
  pageSize: number;
  total: number;
  onPageChange: (nextPage: number) => void;
  className?: string;
}) {
  const { tx, fmtNum } = useI18n();
  const totalPages = Math.max(1, Math.ceil(total / Math.max(1, pageSize)));
  if (total <= pageSize) return null;
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-2 pt-2 text-xs text-muted-foreground",
        className
      )}
    >
      <span>
        {tx(
          `共 ${fmtNum(total, 0)} 条 · 第 ${page} / ${totalPages} 页`,
          `${fmtNum(total, 0)} items · Page ${page} of ${totalPages}`
        )}
      </span>
      <div className="flex items-center gap-1">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={page <= 1}
          onClick={() => onPageChange(page - 1)}
        >
          <ChevronLeft className="h-3.5 w-3.5" />
          {tx("上一页", "Prev")}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={page >= totalPages}
          onClick={() => onPageChange(page + 1)}
        >
          {tx("下一页", "Next")}
          <ChevronRight className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

function useOverlayEscapeAndLock(open: boolean, onClose: () => void): void {
  useEffect(() => {
    if (!open || typeof document === "undefined") return;
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") {
        ev.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prevOverflow;
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);
}

export function Modal({
  open,
  onClose,
  title,
  footer,
  children,
  maxWidthClass = "max-w-lg",
  testId,
  closeTestId,
}: {
  open: boolean;
  onClose: () => void;
  title: React.ReactNode;
  footer?: React.ReactNode;
  children: React.ReactNode;
  maxWidthClass?: string;
  testId?: string;
  closeTestId?: string;
}) {
  const { tx } = useI18n();
  useOverlayEscapeAndLock(open, onClose);
  if (!open || typeof document === "undefined") return null;

  return createPortal(
    <div
      role="dialog"
      aria-modal="true"
      data-testid={testId}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-[1px] p-4"
    >
      <Card
        className={cn(
          "w-full max-h-[88vh] flex flex-col overflow-hidden shadow-xl",
          maxWidthClass
        )}
      >
        <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3 shrink-0">
          <div className="text-sm font-semibold flex items-center gap-2 min-w-0 truncate">
            {title}
          </div>
          <IconButton
            size="sm"
            onClick={onClose}
            data-testid={closeTestId}
            aria-label={tx("关闭", "Close")}
            title={tx("关闭", "Close")}
          >
            <X className="h-4 w-4" />
          </IconButton>
        </div>
        <div className="flex-1 overflow-y-auto p-4 space-y-3 text-xs">
          {children}
        </div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3 bg-muted/20 shrink-0">
            {footer}
          </div>
        )}
      </Card>
    </div>,
    document.body
  );
}

export function Drawer({
  open,
  onClose,
  title,
  footer,
  children,
  widthClass = "max-w-xl",
  testId,
  closeTestId,
  closeAriaLabel,
}: {
  open: boolean;
  onClose: () => void;
  title: React.ReactNode;
  footer?: React.ReactNode;
  children: React.ReactNode;
  widthClass?: string;
  testId?: string;
  closeTestId?: string;
  closeAriaLabel?: string;
}) {
  const { tx } = useI18n();
  useOverlayEscapeAndLock(open, onClose);
  if (!open || typeof document === "undefined") return null;
  const closeLabel = closeAriaLabel ?? tx("关闭", "Close");

  return createPortal(
    <div
      role="dialog"
      aria-modal="true"
      data-testid={testId}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      className="fixed inset-0 z-50 flex justify-end bg-black/45 backdrop-blur-[1px]"
    >
      <div
        className={cn(
          "w-full h-full bg-card text-card-foreground border-l border-border flex flex-col shadow-2xl",
          widthClass
        )}
      >
        <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3 shrink-0">
          <div className="text-sm font-semibold flex items-center gap-2 min-w-0 truncate">
            {title}
          </div>
          <IconButton
            size="sm"
            onClick={onClose}
            data-testid={closeTestId}
            aria-label={closeLabel}
            title={closeLabel}
          >
            <X className="h-4 w-4" />
          </IconButton>
        </div>
        <div className="flex-1 overflow-y-auto p-4 space-y-3 text-xs">
          {children}
        </div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3 bg-muted/20 shrink-0">
            {footer}
          </div>
        )}
      </div>
    </div>,
    document.body
  );
}

export function ConfirmDialog({
  open,
  onClose,
  onConfirm,
  title,
  description,
  confirmLabel,
  danger = true,
  loading = false,
}: {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: React.ReactNode;
  description: React.ReactNode;
  confirmLabel?: React.ReactNode;
  danger?: boolean;
  loading?: boolean;
}) {
  const { tx } = useI18n();
  return (
    <Modal
      open={open}
      onClose={onClose}
      maxWidthClass="max-w-md"
      title={
        <>
          <AlertTriangle
            className={cn(
              "h-4 w-4 shrink-0",
              danger ? "text-red-500" : "text-amber-500"
            )}
          />
          <span>{title}</span>
        </>
      }
      footer={
        <>
          <Button variant="outline" type="button" onClick={onClose} disabled={loading}>
            {tx("取消", "Cancel")}
          </Button>
          <Button
            variant={danger ? "danger" : "primary"}
            type="button"
            onClick={onConfirm}
            disabled={loading}
          >
            {confirmLabel ?? (danger ? tx("删除", "Delete") : tx("确定", "Confirm"))}
          </Button>
        </>
      }
    >
      <p className="text-muted-foreground leading-relaxed">{description}</p>
    </Modal>
  );
}
