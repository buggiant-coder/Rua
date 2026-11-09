import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { Tabs } from "@/ui";
import MCPPage from "../MCP";
import SharedMCPPage from "../../Shared/SharedMCP";
import styles from "./index.module.less";

type McpTabKey = "agent" | "shared";

const VALID_TABS: McpTabKey[] = ["agent", "shared"];

export default function MCPHubPage() {
  const { t } = useTranslation();
  const [headerExtraHost, setHeaderExtraHost] = useState<HTMLDivElement | null>(
    null,
  );
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab") as McpTabKey | null;
  const activeTab: McpTabKey =
    tabParam && VALID_TABS.includes(tabParam) ? tabParam : "agent";

  useEffect(() => {
    if (tabParam && !VALID_TABS.includes(tabParam)) {
      const next = new URLSearchParams(searchParams);
      next.delete("tab");
      setSearchParams(next, { replace: true });
    }
  }, [searchParams, setSearchParams, tabParam]);

  const handleTabChange = (key: string) => {
    const next = key as McpTabKey;
    const params = new URLSearchParams(searchParams);
    if (next === "agent") params.delete("tab");
    else params.set("tab", next);
    setSearchParams(params, { replace: true });
  };

  return (
    <Page className={styles.hubPage}>
      <PageHeader
        current={t("nav.mcp")}
        agentScoped
        extra={<div ref={setHeaderExtraHost} />}
      />
      <PageBody className={styles.pageBody}>
        <Tabs
          className={styles.mainTabs}
          activeKey={activeTab}
          onChange={handleTabChange}
          items={[
            {
              key: "agent",
              label: t("nav.mcp"),
              children: (
                <div className={styles.tabPane}>
                  <MCPPage
                    embedded
                    headerExtraContainer={
                      activeTab === "agent" ? headerExtraHost : null
                    }
                  />
                </div>
              ),
            },
            {
              key: "shared",
              label: t("nav.sharedMcp"),
              children: (
                <div className={styles.tabPane}>
                  <SharedMCPPage
                    embedded
                    headerExtraContainer={
                      activeTab === "shared" ? headerExtraHost : null
                    }
                  />
                </div>
              ),
            },
          ]}
        />
      </PageBody>
    </Page>
  );
}
