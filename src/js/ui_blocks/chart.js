/**
 * chart renderer — DEEIX ChartProps → ECharts option (vanilla port).
 *
 * ChartProps (DEEIX contract):
 *   { title?, type: line|bar|area|horizontal-bar|scatter|pie|radar|funnel|radial,
 *     x?: (string|number)[], series: {name,data,type?,axis?}[], unit?, stacked?, percent? }
 *
 * buildChartOption is pure (unit-tested); the ECharts dependency is a
 * tree-shaken core import (charts/components/renderers used only) — the
 * single-IIFE bundle cannot lazy-load, so per-use size matters.
 */

import * as echarts from 'echarts/core';
import { LineChart, BarChart, ScatterChart, PieChart, RadarChart, FunnelChart } from 'echarts/charts';
import {
  GridComponent, TitleComponent, TooltipComponent, LegendComponent, RadarComponent,
} from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';

echarts.use([
  LineChart, BarChart, ScatterChart, PieChart, RadarChart, FunnelChart,
  GridComponent, TitleComponent, TooltipComponent, LegendComponent, RadarComponent,
  CanvasRenderer,
]);

const PALETTE = ['#4a9eff', '#47c97e', '#e8b84a', '#c678dd', '#ff7b72'];

function seriesTone(s, idx) {
  return { name: String(s && s.name != null ? s.name : `系列${idx + 1}`), tone: PALETTE[idx % PALETTE.length] };
}

function unitSuffix(unit) {
  return unit ? String(unit) : '';
}

function columnSums(seriesData) {
  const len = Math.max(0, ...seriesData.map((d) => d.length));
  const sums = new Array(len).fill(0);
  for (const d of seriesData) {
    for (let i = 0; i < d.length; i++) {
      const v = Number(d[i]);
      if (Number.isFinite(v)) sums[i] += v;
    }
  }
  return sums;
}

/**
 * Build a full ECharts option object from DEEIX ChartProps.
 * Pure — no DOM, no echarts import (safe under vitest).
 */
export function buildChartOption(props) {
  const p = props || {};
  const type = String(p.type || 'line');
  const unit = unitSuffix(p.unit);
  const series = Array.isArray(p.series) ? p.series.filter((s) => s && Array.isArray(s.data)) : [];
  if (!series.length) return null;

  const palette = { color: PALETTE };
  const base = {
    ...palette,
    tooltip: {
      trigger: type === 'pie' || type === 'funnel' || type === 'radial' ? 'item' : 'axis',
      valueFormatter: unit ? (v) => `${v}${unit}` : undefined,
    },
    legend: { top: 0, type: 'scroll' },
    grid: { top: 30, left: 8, right: 12, bottom: 4, containLabel: true },
  };
  if (p.title) base.title = { text: String(p.title), left: 0, top: 2, textStyle: { fontSize: 13 } };

  const xLabels = Array.isArray(p.x) && p.x.length ? p.x.map((v) => String(v)) : null;
  const stacked = !!p.stacked;
  const percent = !!(p.stacked && p.percent);
  const datas = series.map((s) => s.data.map((v) => (v == null ? null : Number(v))));

  const fmt = (v) => {
    if (v == null || !Number.isFinite(Number(v))) return '—';
    const n = Number(v);
    const s = Math.abs(n) >= 1000 ? n.toLocaleString('en-US', { maximumFractionDigits: 2 })
      : String(Math.round(n * 1000) / 1000);
    return percent ? `${Math.round(n)}%` : `${s}${unit}`;
  };

  if (type === 'pie' || type === 'funnel') {
    const labels = xLabels || datas[0].map((_, i) => String(i + 1));
    const data = labels.map((label, i) => ({
      name: label,
      value: Number(datas[0][i] == null ? 0 : datas[0][i]),
    }));
    return {
      ...base,
      series: [{
        type,
        radius: type === 'pie' ? ['38%', '66%'] : '62%',
        center: ['50%', '54%'],
        data,
        label: { formatter: (o) => `${o.name}: ${fmt(o.value)}` },
        emphasis: { focus: 'series' },
      }],
    };
  }

  if (type === 'radial') {
    // DEEIX radial = radial bars; ECharts polar bar approximates RadialBarChart.
    const labels = xLabels || datas[0].map((_, i) => String(i + 1));
    return {
      ...base,
      polar: { radius: ['12%', '76%'], center: ['50%', '52%'] },
      angleAxis: { type: 'category', data: labels, startAngle: 90 },
      radiusAxis: { axisLabel: { formatter: fmt } },
      series: series.map((s, i) => ({
        type: 'bar',
        coordinateSystem: 'polar',
        data: datas[i].map((v) => (v == null ? null : Number(v))),
        name: seriesTone(s, i).name,
        stack: stacked ? 't' : undefined,
      })),
    };
  }

  if (type === 'radar') {
    const labels = xLabels || datas[0].map((_, i) => String(i + 1));
    return {
      ...base,
      radar: {
        indicator: labels.map((name) => ({ name, max: undefined })),
        radius: '66%',
      },
      series: [{
        type: 'radar',
        data: series.map((s, i) => ({
          name: seriesTone(s, i).name,
          value: datas[i].map((v) => (v == null ? null : Number(v))),
          areaStyle: { opacity: 0.18 },
        })),
      }],
    };
  }

  // Cartesian family: line / area / bar / horizontal-bar / scatter
  const horizontal = type === 'horizontal-bar';
  const percentSums = percent ? columnSums(datas) : null;
  const norm = (arr) => (percent
    ? arr.map((v, i) => (v == null || !Number.isFinite(v) ? null : (percentSums[i] ? Math.round((v / percentSums[i]) * 10000) / 100 : 0)))
    : arr);

  // axis factories first — scatter branch below returns early through them
  const catAxis = (extra) => ({
    type: 'category',
    data: xLabels || series[0].data.map((_, i) => String(i + 1)),
    axisLine: { lineStyle: { color: 'rgba(127,127,127,.4)' } },
    axisLabel: { hideOverlap: true },
    ...extra,
  });
  const valAxis = (extra) => ({
    type: 'value',
    axisLabel: { formatter: fmt },
    splitLine: { lineStyle: { color: 'rgba(127,127,127,.15)' } },
    ...extra,
  });

  if (type === 'scatter') {
    // DEEIX scatter: x values are numeric, series.data are y values.
    const xNums = xLabels ? xLabels.map((v) => Number(v)) : null;
    return {
      ...base,
      xAxis: valAxis(),
      yAxis: valAxis(),
      series: series.map((s, i) => ({
        name: seriesTone(s, i).name,
        type: 'scatter',
        symbolSize: 9,
        data: datas[i].map((y, k) => {
          const x = xNums && Number.isFinite(xNums[k]) ? xNums[k] : k + 1;
          return y == null || !Number.isFinite(y) ? null : [x, y];
        }).filter(Boolean),
        emphasis: { focus: 'series' },
      })),
    };
  }

  const esSeries = series.map((s, i) => {
    const declared = s.type === 'bar' || s.type === 'line' || s.type ? String(s.type) : null;
    let sType;
    if (type === 'scatter') sType = 'scatter';
    else if (type === 'bar' || type === 'horizontal-bar') sType = declared === 'line' ? 'line' : 'bar';
    else sType = declared === 'bar' ? 'bar' : 'line';
    const isArea = type === 'area' || (type === 'line' && declared === 'area');
    const out = {
      name: seriesTone(s, i).name,
      type: sType,
      data: norm(datas[i]),
      smooth: sType === 'line' ? 0.25 : undefined,
      showSymbol: type === 'scatter' || datas[i].length <= 24,
      areaStyle: isArea ? { opacity: 0.16 } : undefined,
      stack: stacked && sType !== 'scatter' ? 't' : undefined,
      yAxisIndex: !horizontal && s.axis === 'right' ? 1 : undefined,
      emphasis: { focus: 'series' },
    };
    return out;
  });

  const hasRight = !horizontal && series.some((s) => s && s.axis === 'right');
  const opt = {
    ...base,
    xAxis: horizontal ? valAxis() : catAxis({ type: 'category' }),
    yAxis: horizontal
      ? catAxis({ type: 'category', data: xLabels || series[0].data.map((_, i) => String(i + 1)) })
      : (hasRight
        ? [valAxis(), valAxis({ position: 'right', splitLine: { show: false } })]
        : valAxis()),
    series: esSeries,
  };
  // horizontal: category axis must be yAxis, value axis is xAxis
  if (horizontal) {
    opt.xAxis = valAxis();
    opt.yAxis = catAxis({ type: 'category', data: xLabels || series[0].data.map((_, i) => String(i + 1)) });
  }
  if (type === 'area' && !esSeries.some((s) => s.areaStyle)) {
    for (const s of esSeries) if (s.type === 'line') s.areaStyle = { opacity: 0.16 };
  }
  return opt;
}

/** Mount an echarts chart into `container` (core is bundled tree-shaken). */
export async function mountChart(container, props) {
  const option = buildChartOption(props);
  if (!option) {
    const empty = document.createElement('div');
    empty.className = 'stui-empty';
    empty.textContent = '（图表无数据）';
    container.appendChild(empty);
    return null;
  }
  const inst = echarts.init(container);
  inst.setOption(option);
  const onResize = () => inst.resize();
  window.addEventListener('resize', onResize);
  // dispose when node leaves the DOM (bubble re-render / session switch)
  const observer = new MutationObserver(() => {
    if (!document.contains(container)) {
      window.removeEventListener('resize', onResize);
      observer.disconnect();
      inst.dispose();
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
  return inst;
}
