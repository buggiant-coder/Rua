import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { Tabs } from "@/ui";
import SkillsPage from "../Skills";
import SkillPoolPage from "../../Shared/SkillPool";
import MarketPage from "../../Settings/Market";
import styles from "./index.module.less";

type SkillTabKey = "installed" | "pool" | "market";

const VALID_TABS: SkillTabKey[] = ["installed", "pool", "market"];

export default function SkillsHubPage() {
  const { t } = useTranslation();
  const [headerExtraHost, setHeaderExtraHost] = useState<HTMLDivElement | null>(
    null,
  );
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab") as SkillTabKey | null;
  const activeTab: SkillTabKey =
    tabParam && VALID_TABS.includes(tabParam) ? tabParam : "installed";

  useEffect(() => {
    if (tabParam && !VALID_TABS.includes(tabParam)) {
      const next = new URLSearchParams(searchParams);
      next.delete("tab");
      setSearchParams(next, { replace: true });
    }
  }, [searchParams, setSearchParams, tabParam]);

  const handleTabChange = (key: string) => {
    const next = key as SkillTabKey;
    const params = new URLSearchParams(searchParams);
    if (next === "installed") params.delete("tab");
    else params.set("tab", next);
    setSearchParams(params, { replace: true });
  };

  return (
    <Page className={styles.hubPage}>
      <PageHeader
        current={t("nav.skills")}
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
              key: "installed",
              label: t("nav.skills"),
              children: (
                <div className={styles.tabPane}>
                  <SkillsPage
                    embedded
                    headerExtraContainer={
                      activeTab === "installed" ? headerExtraHost : null
                    }
                  />
                </div>
              ),
            },
            {
              key: "pool",
              label: t("nav.skillPool"),
              children: (
                <div className={styles.tabPane}>
                  <SkillPoolPage
                    embedded
                    headerExtraContainer={
                      activeTab === "pool" ? headerExtraHost : null
                    }
                  />
                </div>
              ),
            },
            {
              key: "market",
              label: t("nav.market"),
              children: (
                <div className={styles.tabPane}>
                  <MarketPage
                    embedded
                    headerExtraContainer={
                      activeTab === "market" ? headerExtraHost : null
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
