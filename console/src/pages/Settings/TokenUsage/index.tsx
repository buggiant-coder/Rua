/* eslint-disable react-refresh/only-export-components */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useLoaderData } from "react-router-dom";

import { RuaCard } from "@/components/RuaCard";
import { useTranslation } from "react-i18next";
import dayjs, { Dayjs } from "dayjs";
import api from "../../../api";
import type {
  TokenUsageRecord,
  TokenUsageSummary,
  TokenUsageStats,
} from "../../../api/types/tokenUsage";
import { formatCompact } from "../../../utils/formatNumber";
import { LoadingState } from "./components";
import { EmptyState } from "@/components/EmptyState";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import styles from "./index.module.less";
import { DatePicker, Table, Tag, toast } from "@/ui";

type ByModelRow = TokenUsageStats & { key: string };
type ByDateRow = TokenUsageStats & { key: string; date: string };
type ByDateModelMap = Record<string, Record<string, ByModelRow>>;
type AggregatedUsage = TokenUsageSummary & { by_date_model: ByDateModelMap };

const CHART_COLORS = [
  "#2563eb",
  "#16a34a",
  "#f59e0b",
  "#dc2626",
  "#0891b2",
  "#7c3aed",
];

function aggregateRecords(records: TokenUsageRecord[]): AggregatedUsage {
  const summary: AggregatedUsage = {
    total_prompt_tokens: 0,
    total_completion_tokens: 0,
    total_calls: 0,
    by_model: {},
    by_date: {},
    by_date_model: {},
  };

  for (const record of records) {
    const prompt = record.prompt_tokens || 0;
    const completion = record.completion_tokens || 0;
    const calls = record.call_count || 0;
    summary.total_prompt_tokens += prompt;
    summary.total_completion_tokens += completion;
    summary.total_calls += calls;

    const modelKey = record.provider_id
      ? `${record.provider_id}:${record.model}`
      : record.model;
    if (!summary.by_model[modelKey]) {
      summary.by_model[modelKey] = {
        provider_id: record.provider_id,
        model: record.model,
        prompt_tokens: 0,
        completion_tokens: 0,
        call_count: 0,
      };
    }
    summary.by_model[modelKey].prompt_tokens += prompt;
    summary.by_model[modelKey].completion_tokens += completion;
    summary.by_model[modelKey].call_count += calls;

    if (!summary.by_date_model[record.date]) {
      summary.by_date_model[record.date] = {};
    }
    if (!summary.by_date_model[record.date][modelKey]) {
      summary.by_date_model[record.date][modelKey] = {
        key: modelKey,
        provider_id: record.provider_id,
        model: record.model,
        prompt_tokens: 0,
        completion_tokens: 0,
        call_count: 0,
      };
    }
    summary.by_date_model[record.date][modelKey].prompt_tokens += prompt;
    summary.by_date_model[record.date][modelKey].completion_tokens +=
      completion;
    summary.by_date_model[record.date][modelKey].call_count += calls;

    if (!summary.by_date[record.date]) {
      summary.by_date[record.date] = {
        prompt_tokens: 0,
        completion_tokens: 0,
        call_count: 0,
      };
    }
    summary.by_date[record.date].prompt_tokens += prompt;
    summary.by_date[record.date].completion_tokens += completion;
    summary.by_date[record.date].call_count += calls;
  }

  return summary;
}

function dateRange(startDate: Dayjs, endDate: Dayjs): string[] {
  const dates: string[] = [];
  let cursor = startDate.startOf("day");
  const end = endDate.startOf("day");
  while (cursor.isBefore(end) || cursor.isSame(end, "day")) {
    dates.push(cursor.format("YYYY-MM-DD"));
    cursor = cursor.add(1, "day");
  }
  return dates;
}

function linePoints(values: number[], maxValue: number): string {
  if (values.length === 0) return "";
  const width = 100;
  const height = 100;
  const denominator = Math.max(values.length - 1, 1);
  return values
    .map((value, index) => {
      const x = (index / denominator) * width;
      const y = height - (value / Math.max(maxValue, 1)) * height;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

function TrendChart({
  title,
  series,
}: {
  title: string;
  series: Array<{ name: string; values: number[]; color: string }>;
}) {
  const maxValue = Math.max(...series.flatMap((item) => item.values), 0);
  if (series.length === 0 || maxValue === 0) return null;

  return (
    <RuaCard className={`${styles.card} ${styles.chartCard}`}>
      <div className={styles.chartHeader}>
        <h3 className={styles.listSectionTitle}>{title}</h3>
        <span className={styles.chartPeak}>{formatCompact(maxValue)}</span>
      </div>
      <svg
        className={styles.lineChart}
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <line x1="0" y1="100" x2="100" y2="100" className={styles.axisLine} />
        <line x1="0" y1="50" x2="100" y2="50" className={styles.gridLine} />
        {series.map((item) => (
          <polyline
            key={item.name}
            points={linePoints(item.values, maxValue)}
            fill="none"
            stroke={item.color}
            strokeWidth="2.5"
            vectorEffect="non-scaling-stroke"
          />
        ))}
      </svg>
      <div className={styles.chartLegend}>
        {series.map((item) => (
          <span key={item.name} className={styles.legendItem}>
            <span
              className={styles.legendSwatch}
              style={{ backgroundColor: item.color }}
            />
            {item.name}
          </span>
        ))}
      </div>
    </RuaCard>
  );
}

interface TokenUsageInitialData {
  startDate: string;
  endDate: string;
  data: AggregatedUsage;
}

interface TokenUsagePageProps {
  initialData?: TokenUsageInitialData;
}

export async function loader(): Promise<TokenUsageInitialData> {
  const endDate = dayjs();
  const startDate = endDate.subtract(30, "day");
  const records = await api.getTokenUsageDetails({
    start_date: startDate.format("YYYY-MM-DD"),
    end_date: endDate.format("YYYY-MM-DD"),
  });
  return {
    startDate: startDate.format("YYYY-MM-DD"),
    endDate: endDate.format("YYYY-MM-DD"),
    data: aggregateRecords(records),
  };
}

export function RouteComponent() {
  return (
    <TokenUsagePage initialData={useLoaderData() as TokenUsageInitialData} />
  );
}

function TokenUsagePage({ initialData }: TokenUsagePageProps) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(initialData === undefined);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<AggregatedUsage | null>(
    initialData?.data ?? null,
  );
  const [startDate, setStartDate] = useState<Dayjs>(
    initialData ? dayjs(initialData.startDate) : dayjs().subtract(30, "day"),
  );
  const [endDate, setEndDate] = useState<Dayjs>(
    initialData ? dayjs(initialData.endDate) : dayjs(),
  );

  useEffect(() => {
    if (!initialData) return;
    setStartDate(dayjs(initialData.startDate));
    setEndDate(dayjs(initialData.endDate));
    setData(initialData.data);
    setError(null);
    setLoading(false);
  }, [initialData]);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const records = await api.getTokenUsageDetails({
        start_date: startDate.format("YYYY-MM-DD"),
        end_date: endDate.format("YYYY-MM-DD"),
      });
      setData(aggregateRecords(records));
    } catch (e) {
      console.error("Failed to load token usage:", e);
      const msg = t("tokenUsage.loadFailed");
      toast.error(msg);
      setError(msg);
      setData(null);
    } finally {
      setLoading(false);
    }
  }, [endDate, startDate, t]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const handleDateChange = (dates: [Dayjs | null, Dayjs | null] | null) => {
    if (dates?.[0]) setStartDate(dates[0]);
    if (dates?.[1]) setEndDate(dates[1]);
  };

  const byModelDataSource: ByModelRow[] = useMemo(() => {
    if (!data?.by_model) return [];
    return Object.entries(data.by_model).map(([key, stats]) => ({
      ...stats,
      key,
    }));
  }, [data?.by_model]);

  const byDateDataSource: ByDateRow[] = useMemo(() => {
    if (!data?.by_date) return [];
    return Object.entries(data.by_date)
      .map(([dt, stats]) => ({
        ...stats,
        key: dt,
        date: dt,
      }))
      .sort((a, b) => b.date.localeCompare(a.date));
  }, [data?.by_date]);

  const chartDates = useMemo(
    () => dateRange(startDate, endDate),
    [endDate, startDate],
  );

  const modelTrendSeries = useMemo(() => {
    if (!data?.by_date_model) return [];
    return Object.entries(data.by_model)
      .sort(
        ([, left], [, right]) =>
          right.prompt_tokens +
          right.completion_tokens -
          (left.prompt_tokens + left.completion_tokens),
      )
      .slice(0, 6)
      .map(([key], index) => ({
        name: key,
        color: CHART_COLORS[index % CHART_COLORS.length],
        values: chartDates.map((date) => {
          const stats = data.by_date_model[date]?.[key];
          return stats ? stats.prompt_tokens + stats.completion_tokens : 0;
        }),
      }));
  }, [chartDates, data?.by_date_model, data?.by_model]);

  const tokenTypeSeries = useMemo(() => {
    if (!data?.by_date) return [];
    return [
      {
        name: t("tokenUsage.promptTokens"),
        color: CHART_COLORS[0],
        values: chartDates.map(
          (date) => data.by_date[date]?.prompt_tokens ?? 0,
        ),
      },
      {
        name: t("tokenUsage.completionTokens"),
        color: CHART_COLORS[1],
        values: chartDates.map(
          (date) => data.by_date[date]?.completion_tokens ?? 0,
        ),
      },
      {
        name: t("tokenUsage.totalTokens"),
        color: CHART_COLORS[2],
        values: chartDates.map((date) => {
          const stats = data.by_date[date];
          return stats ? stats.prompt_tokens + stats.completion_tokens : 0;
        }),
      },
    ];
  }, [chartDates, data?.by_date, t]);

  const renderMetricTag = (label: string, value: number) => (
    <Tag className={styles.metricTag}>
      <span className={styles.metricLabel}>{label}</span>
      <b>{formatCompact(value)}</b>
    </Tag>
  );

  const modelColumns = useMemo(
    () => [
      {
        title: t("tokenUsage.model"),
        key: "model",
        render: (_: unknown, row: ByModelRow) => (
          <span className={styles.tableTitle}>
            <span className={styles.providerName}>
              {row.provider_id ?? "-"}
            </span>
            <span className={styles.titleSeparator}>/</span>
            <span className={styles.modelName}>{row.model ?? row.key}</span>
          </span>
        ),
      },
      {
        title: t("tokenUsage.totalCalls"),
        dataIndex: "call_count",
        key: "call_count",
        width: 130,
        align: "right" as const,
        sorter: (a: ByModelRow, b: ByModelRow) => a.call_count - b.call_count,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.totalCalls"), value),
      },
      {
        title: t("tokenUsage.promptTokens"),
        dataIndex: "prompt_tokens",
        key: "prompt_tokens",
        width: 160,
        align: "right" as const,
        sorter: (a: ByModelRow, b: ByModelRow) =>
          a.prompt_tokens - b.prompt_tokens,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.promptTokens"), value),
      },
      {
        title: t("tokenUsage.completionTokens"),
        dataIndex: "completion_tokens",
        key: "completion_tokens",
        width: 160,
        align: "right" as const,
        sorter: (a: ByModelRow, b: ByModelRow) =>
          a.completion_tokens - b.completion_tokens,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.completionTokens"), value),
      },
    ],
    [t],
  );

  const dateColumns = useMemo(
    () => [
      {
        title: t("tokenUsage.date"),
        dataIndex: "date",
        key: "date",
        render: (value: string) => (
          <span className={styles.dateName}>{value}</span>
        ),
      },
      {
        title: t("tokenUsage.totalCalls"),
        dataIndex: "call_count",
        key: "call_count",
        width: 130,
        align: "right" as const,
        sorter: (a: ByDateRow, b: ByDateRow) => a.call_count - b.call_count,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.totalCalls"), value),
      },
      {
        title: t("tokenUsage.promptTokens"),
        dataIndex: "prompt_tokens",
        key: "prompt_tokens",
        width: 160,
        align: "right" as const,
        sorter: (a: ByDateRow, b: ByDateRow) =>
          a.prompt_tokens - b.prompt_tokens,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.promptTokens"), value),
      },
      {
        title: t("tokenUsage.completionTokens"),
        dataIndex: "completion_tokens",
        key: "completion_tokens",
        width: 160,
        align: "right" as const,
        sorter: (a: ByDateRow, b: ByDateRow) =>
          a.completion_tokens - b.completion_tokens,
        render: (value: number) =>
          renderMetricTag(t("tokenUsage.completionTokens"), value),
      },
    ],
    [t],
  );

  return (
    <Page className={styles.tokenUsagePage}>
      <PageHeader
        current={t("tokenUsage.title")}
        onRefresh={fetchData}
        refreshing={loading}
      />
      <PageBody className={styles.content}>
        <div className={styles.toolbar}>
          <DatePicker.RangePicker
            value={[startDate, endDate]}
            onChange={handleDateChange}
            disabledDate={(current) => current.isAfter(dayjs(), "day")}
            className={styles.datePicker}
            popupClassName={styles.datePickerPopup}
          />
        </div>
        {loading && !data ? (
          <LoadingState
            message={error ?? t("common.loading")}
            error={!!error}
            onRetry={error ? fetchData : undefined}
          />
        ) : (
          <>
            {data && data.total_calls > 0 ? (
              <>
                <div className={styles.summaryCards}>
                  <RuaCard className={`${styles.card} ${styles.kpiCard}`}>
                    <div className={styles.cardValue}>
                      {formatCompact(data.total_calls)}
                    </div>
                    <div className={styles.cardLabel}>
                      {t("tokenUsage.totalCalls")}
                    </div>
                  </RuaCard>
                  <RuaCard className={`${styles.card} ${styles.kpiCard}`}>
                    <div className={styles.cardValue}>
                      {formatCompact(data.total_prompt_tokens)}
                    </div>
                    <div className={styles.cardLabel}>
                      {t("tokenUsage.promptTokens")}
                    </div>
                  </RuaCard>
                  <RuaCard className={`${styles.card} ${styles.kpiCard}`}>
                    <div className={styles.cardValue}>
                      {formatCompact(data.total_completion_tokens)}
                    </div>
                    <div className={styles.cardLabel}>
                      {t("tokenUsage.completionTokens")}
                    </div>
                  </RuaCard>
                  <RuaCard className={`${styles.card} ${styles.kpiCard}`}>
                    <div className={styles.cardValue}>
                      {formatCompact(
                        data.total_prompt_tokens + data.total_completion_tokens,
                      )}
                    </div>
                    <div className={styles.cardLabel}>
                      {t("tokenUsage.totalTokens")}
                    </div>
                  </RuaCard>
                </div>

                <div className={styles.chartGrid}>
                  <TrendChart
                    title={t("tokenUsage.modelTrend")}
                    series={modelTrendSeries}
                  />
                  <TrendChart
                    title={t("tokenUsage.tokenTypeChart")}
                    series={tokenTypeSeries}
                  />
                </div>

                <section className={styles.listSection}>
                  <h3 className={styles.listSectionTitle}>
                    {t("tokenUsage.byModel")}
                  </h3>
                  {byModelDataSource.length > 0 ? (
                    <Table
                      columns={modelColumns}
                      dataSource={byModelDataSource}
                      rowKey="key"
                      pagination={false}
                      responsive="stack"
                      className={styles.usageTable}
                    />
                  ) : (
                    <EmptyState description={t("tokenUsage.byModelEmpty")} />
                  )}
                </section>

                <section className={styles.listSection}>
                  <h3 className={styles.listSectionTitle}>
                    {t("tokenUsage.byDate")}
                  </h3>
                  {byDateDataSource.length > 0 ? (
                    <Table
                      columns={dateColumns}
                      dataSource={byDateDataSource}
                      rowKey="key"
                      pagination={false}
                      responsive="stack"
                      className={styles.usageTable}
                    />
                  ) : (
                    <EmptyState description={t("tokenUsage.byDateEmpty")} />
                  )}
                </section>
              </>
            ) : (
              <EmptyState description={t("tokenUsage.noData")} />
            )}
          </>
        )}
      </PageBody>
    </Page>
  );
}

export default TokenUsagePage;
