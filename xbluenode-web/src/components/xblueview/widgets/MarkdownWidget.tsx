import Markdown from "react-markdown";
import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { readString } from "../utils";

export interface MarkdownWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
}

export function MarkdownWidget({ widget, data }: MarkdownWidgetProps) {
  const { tx } = useI18n();
  const fromOptions =
    readString(widget.options, "content") ??
    readString(widget.options, "markdown") ??
    readString(widget.options, "text");

  const fromStaticRow =
    data?.rows?.[0]?.[0] !== undefined && data.rows[0][0] !== null
      ? String(data.rows[0][0])
      : undefined;

  const content =
    fromOptions ?? fromStaticRow ?? tx("_暂无说明内容_", "_No note content_");

  return (
    <div
      className="h-full overflow-y-auto text-xs leading-relaxed space-y-2 pr-1"
      data-testid={`widget-markdown-${widget.id}`}
    >
      <Markdown
        components={{
          h1: ({ children }) => (
            <h3 className="text-sm font-semibold border-b border-border pb-1">
              {children}
            </h3>
          ),
          h2: ({ children }) => (
            <h4 className="text-xs font-semibold text-foreground">{children}</h4>
          ),
          p: ({ children }) => (
            <p className="text-xs text-muted-foreground leading-relaxed">
              {children}
            </p>
          ),
          ul: ({ children }) => (
            <ul className="list-disc pl-4 space-y-0.5 text-muted-foreground">
              {children}
            </ul>
          ),
          ol: ({ children }) => (
            <ol className="list-decimal pl-4 space-y-0.5 text-muted-foreground">
              {children}
            </ol>
          ),
          code: ({ children }) => (
            <code className="rounded bg-muted px-1 py-0.5 font-mono-tech text-[11px] text-foreground">
              {children}
            </code>
          ),
          a: ({ href, children }) => (
            <a
              href={href}
              target="_blank"
              rel="noopener noreferrer"
              className="text-xblue underline underline-offset-2"
            >
              {children}
            </a>
          ),
        }}
      >
        {content}
      </Markdown>
    </div>
  );
}
