import React from "react";
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

export function Button({
  variant = "primary",
  className,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "outline" | "ghost";
}) {
  return (
    <button
      className={cn(
        "inline-flex items-center justify-center gap-2 rounded-md px-3.5 py-2 text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-40 cursor-pointer",
        variant === "primary" &&
          "bg-primary text-primary-foreground hover:opacity-90",
        variant === "outline" &&
          "border border-border bg-background hover:bg-muted text-foreground",
        variant === "ghost" && "hover:bg-muted text-muted-foreground hover:text-foreground",
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
  variant?: "neutral" | "xblue" | "outline";
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium font-mono-tech",
        variant === "neutral" && "bg-muted text-foreground",
        variant === "xblue" && "bg-xblue-soft text-xblue border border-xblue/25",
        variant === "outline" && "border border-border text-muted-foreground",
        className
      )}
      {...props}
    />
  );
}

export function Input({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-foreground",
        className
      )}
      {...props}
    />
  );
}

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        "flex w-full rounded-md border border-input bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-foreground",
        className
      )}
      {...props}
    />
  );
}
