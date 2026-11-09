import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { Tabs } from "@/ui";
import WorkspacePage from "../Workspace";
import PromptLibraryPage from "../../Shared/PromptLibrary";
import styles from "./index.module.less";

type PromptTabKey = "workspace" | "library";

const VALID_TABS: PromptTabKey[] = ["workspace", "library"];

export default function PromptsHubPage() {
  const { t } = useTranslation();
  const [headerExtraHost, setHeaderExtraHost] = useState<HTMLDivElement | null>(
    null,
  );
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab") as PromptTabKey | null;
  const activeTab: PromptTabKey =
    tabParam && VALID_TABS.includes(tabParam) ? tabParam : "workspace";

  useEffect(() => {
    if (tabParam && !VALID_TABS.includes(tabParam)) {
      const next = new URLSearchParams(searchParams);
      next.delete("tab");
      setSearchParams(next, { replace: true });
    }
  }, [searchParams, setSearchParams, tabParam]);

  const handleTabChange = (key: string) => {
    const next = key as PromptTabKey;
    const params = new URLSearchParams(searchParams);
    if (next === "workspace") params.delete("tab");
    else params.set("tab", next);
    setSearchParams(params, { replace: true });
  };

  return (
    <Page className={styles.hubPage}>
      <PageHeader
        current={t("nav.workspace")}
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
              key: "workspace",
              label: t("nav.workspace"),
              children: (
                <div className={styles.tabPane}>
                  <WorkspacePage
                    embedded
                    headerExtraContainer={
                      activeTab === "workspace" ? headerExtraHost : null
                    }
                  />
                </div>
              ),
            },
            {
              key: "library",
              label: t("nav.promptLibrary"),
              children: (
                <div className={styles.tabPane}>
                  <PromptLibraryPage
                    embedded
                    headerExtraContainer={
                      activeTab === "library" ? headerExtraHost : null
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
