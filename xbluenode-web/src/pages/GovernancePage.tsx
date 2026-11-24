import { useSearchParams } from "react-router";
import { ServerCog, ShieldCheck } from "lucide-react";
import { PageHeader, SegmentedControl } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { AxGuardPage } from "@/pages/AxGuardPage";
import { AxOpsPage } from "@/pages/AxOpsPage";

type GovernanceSection = "security" | "ops";

const OPS_TABS = new Set([
  "health",
  "jobs",
  "retention",
  "backups",
  "templates",
  "work_items",
  "settings",
]);

function resolveGovernanceSection(
  rawSection: string | null,
  rawTab: string | null
): GovernanceSection {
  if (rawSection === "ops") return "ops";
  if (rawSection === "security") return "security";
  const normalizedTab = (rawTab ?? "").trim().toLowerCase();
  if (OPS_TABS.has(normalizedTab)) {
    return "ops";
  }
  return "security";
}

export function GovernancePage() {
  const { tx } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const rawSection = searchParams.get("section");
  const rawTab = searchParams.get("tab");
  const section = resolveGovernanceSection(rawSection, rawTab);

  const handleSectionChange = (next: GovernanceSection) => {
    const updated = new URLSearchParams(searchParams);
    updated.set("section", next);
    updated.delete("tab");
    setSearchParams(updated, { replace: true });
  };

  return (
    <div className="space-y-4" data-testid="governance-page">
      <PageHeader
        title={tx(MODULE_META.governance.label, MODULE_META.governance.enLabel)}
        actions={
          <SegmentedControl<GovernanceSection>
            value={section}
            onChange={handleSectionChange}
            items={[
              {
                key: "security",
                label: tx("权限与安全", "Access & Security"),
                icon: <ShieldCheck className="h-3.5 w-3.5 text-xblue" />,
                testId: "governance-tab-security",
              },
              {
                key: "ops",
                label: tx("运维与备份", "Operations & Backup"),
                icon: <ServerCog className="h-3.5 w-3.5 text-xblue" />,
                testId: "governance-tab-ops",
              },
            ]}
          />
        }
      />

      {section === "ops" ? <AxOpsPage embedded /> : <AxGuardPage embedded />}
    </div>
  );
}
