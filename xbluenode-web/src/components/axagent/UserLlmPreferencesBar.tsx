import { useMemo, useState } from "react";
import { SlidersHorizontal } from "lucide-react";
import {
  putUserLlmPreferences,
  type LlmProviderRecord,
  type LlmPurpose,
  type UserLlmPreferenceItem,
  type UserLlmPreferenceRecord,
} from "@/lib/api/axagent";
import {
  Badge,
  Button,
  Card,
  Select,
  type SelectOptionItem,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

const ALL_PURPOSES: readonly LlmPurpose[] = [
  "AGENT",
  "CHAT",
  "NL2SQL",
  "SUMMARIZE",
  "EXTRACT",
  "EMBED",
];

export interface UserLlmPreferencesBarProps {
  providers: LlmProviderRecord[];
  preferences: UserLlmPreferenceRecord[];
  onRefresh: () => Promise<void>;
}

export function UserLlmPreferencesBar({
  providers,
  preferences,
  onRefresh,
}: UserLlmPreferencesBarProps) {
  const { tx } = useI18n();

  const [draft, setDraft] = useState<Partial<Record<LlmPurpose, string>>>({});
  const [saving, setSaving] = useState(false);
  const [saveStatus, setSaveStatus] = useState<"idle" | "saved" | "failed">(
    "idle"
  );

  const serverMap = useMemo(() => {
    const next: Partial<Record<LlmPurpose, string>> = {};
    for (const row of preferences) {
      next[row.purpose] = row.provider_id;
    }
    return next;
  }, [preferences]);

  const sortedEnabledProviders = useMemo(
    () =>
      providers
        .filter((p) => p.enabled)
        .sort((a, b) => a.priority - b.priority),
    [providers]
  );

  const optionsByPurpose = useMemo(() => {
    const sysDefaultLabel = tx("系统默认", "System Default");
    const personalGroupLabel = tx("个人模型", "Personal");
    const systemGroupLabel = tx("系统模型", "System");

    const personalProviders = sortedEnabledProviders.filter((p) =>
      Boolean(p.owner_user_id)
    );
    const systemProviders = sortedEnabledProviders.filter(
      (p) => !p.owner_user_id
    );

    const map: Record<LlmPurpose, SelectOptionItem[]> = {
      AGENT: [],
      CHAT: [],
      NL2SQL: [],
      SUMMARIZE: [],
      EXTRACT: [],
      EMBED: [],
    };
    for (const purpose of ALL_PURPOSES) {
      const opts: SelectOptionItem[] = [
        {
          value: "",
          label: sysDefaultLabel,
          text: sysDefaultLabel,
        },
      ];
      for (const p of personalProviders) {
        if (p.purposes.includes(purpose)) {
          const text = `${p.name} · ${p.model}`;
          opts.push({
            value: p.id,
            label: text,
            text,
            group: personalGroupLabel,
          });
        }
      }
      for (const p of systemProviders) {
        if (p.purposes.includes(purpose)) {
          const text = `${p.name} · ${p.model}`;
          opts.push({
            value: p.id,
            label: text,
            text,
            group: systemGroupLabel,
          });
        }
      }
      map[purpose] = opts;
    }
    return map;
  }, [sortedEnabledProviders, tx]);

  const getSelectedValue = (purpose: LlmPurpose): string => {
    const candidate =
      draft[purpose] !== undefined
        ? draft[purpose]
        : (serverMap[purpose] ?? "");
    const validOptions = optionsByPurpose[purpose];
    return validOptions.some((o) => o.value === candidate) ? candidate : "";
  };

  const handleChangePurpose = (purpose: LlmPurpose, nextProviderId: string) => {
    setSaveStatus("idle");
    setDraft((prev) => ({
      ...prev,
      [purpose]: nextProviderId,
    }));
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveStatus("idle");
    const items: UserLlmPreferenceItem[] = [];
    for (const purpose of ALL_PURPOSES) {
      const selectedId = getSelectedValue(purpose);
      if (selectedId.length > 0) {
        items.push({ purpose, provider_id: selectedId });
      }
    }
    try {
      await putUserLlmPreferences({ preferences: items });
      setDraft({});
      await onRefresh();
      setSaveStatus("saved");
    } catch {
      setSaveStatus("failed");
    } finally {
      setSaving(false);
    }
  };

  return (
    <Card data-testid="user-llm-preferences-bar" className="p-4 space-y-3.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <SlidersHorizontal className="h-4 w-4 text-xblue shrink-0" />
          <h2 className="text-sm font-semibold">
            {tx("默认模型", "Default Models")}
          </h2>
          {saveStatus === "saved" ? (
            <Badge variant="success" data-testid="llm-pref-status">
              {tx("已保存", "Saved")}
            </Badge>
          ) : saveStatus === "failed" ? (
            <Badge variant="danger" data-testid="llm-pref-status">
              {tx("保存失败", "Failed")}
            </Badge>
          ) : null}
        </div>

        <Button
          type="button"
          size="sm"
          data-testid="llm-pref-save"
          disabled={saving}
          onClick={() => void handleSave()}
        >
          {saving
            ? tx("保存中…", "Saving…")
            : tx("保存偏好", "Save")}
        </Button>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {ALL_PURPOSES.map((purpose) => (
          <div key={purpose} className="space-y-1">
            <label className="block font-mono-tech text-[11px] font-medium text-muted-foreground">
              {purpose}
            </label>
            <Select
              className="w-full"
              aria-label={purpose}
              data-testid={`llm-pref-select-${purpose}`}
              value={getSelectedValue(purpose)}
              options={optionsByPurpose[purpose]}
              onChange={(e) => handleChangePurpose(purpose, e.target.value)}
            />
          </div>
        ))}
      </div>
    </Card>
  );
}
