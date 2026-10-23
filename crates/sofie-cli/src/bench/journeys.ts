import { Rate, Trend } from 'k6/metrics';
import { type Sessions, signInAll } from './client.ts';

interface Journey {
  name: string;
  metric: string;
  run: (sessions: Sessions) => void;
}

const VUS = 16;
const SECONDS = 10;

const measured = Object.fromEntries(
  JOURNEYS.map((journey) => [
    journey.name,
    {
      latency: new Trend(`latency_${journey.metric}`, true),
      failed: new Rate(`failed_${journey.metric}`),
    },
  ]),
);

// One journey after another, so each one's p95 is its own.
export const options = {
  scenarios: Object.fromEntries(
    JOURNEYS.map((journey, index) => [
      journey.metric,
      {
        executor: 'constant-vus',
        vus: VUS,
        duration: `${SECONDS}s`,
        startTime: `${index * (SECONDS + 1)}s`,
        gracefulStop: '1s',
        exec: 'walk',
        env: { SOFIE_BENCH_JOURNEY: String(index) },
      },
    ]),
  ),
};

export function setup(): { sessions: Sessions } {
  return { sessions: signInAll() };
}

// A journey fails as a whole when one of its requests is refused.
export function walk(data: { sessions: Sessions }): void {
  const journey = JOURNEYS[Number(__ENV.SOFIE_BENCH_JOURNEY)];
  const started = Date.now();
  let failed = false;
  try {
    journey.run(data.sessions);
  } catch (error) {
    failed = true;
    console.warn(`${journey.name}: ${error}`);
  }
  measured[journey.name].latency.add(Date.now() - started);
  measured[journey.name].failed.add(failed);
}

export function handleSummary(data: { metrics: Record<string, { values: Record<string, number> }> }) {
  const routes: Record<string, { p95_ms: number; error_rate: number }> = {};
  for (const journey of JOURNEYS) {
    const latency = data.metrics[`latency_${journey.metric}`];
    const failed = data.metrics[`failed_${journey.metric}`];
    if (latency !== undefined && failed !== undefined) {
      routes[journey.name] = { p95_ms: latency.values['p(95)'], error_rate: failed.values.rate };
    }
  }
  return { [__ENV.SOFIE_BENCH_SUMMARY]: JSON.stringify({ routes }) };
}
