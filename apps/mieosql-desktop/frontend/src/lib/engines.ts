// Engine metadata + cloud presets — the single source of truth for the
// connection dialog: labels, grouping, default ports, per-engine form hints
// and provider presets.
//
// `variant` is descriptive only: the backend driver is always chosen by
// `engine` (e.g. Redshift and Aurora PostgreSQL both run the postgres driver).

import type { EngineKind } from "@bindings/EngineKind";

import type { Engine, SslMode } from "./bindings";

export type EngineGroup = "SQL" | "NoSQL" | "Message broker";

/** Display order of the engine picker's <optgroup>s. */
export const ENGINE_GROUPS: readonly EngineGroup[] = [
  "SQL",
  "NoSQL",
  "Message broker",
];

export type EngineMeta = {
  engine: Engine;
  label: string;
  group: EngineGroup;
  kind: EngineKind;
  /** TCP port prefilled when the engine is selected; `null` for file engines. */
  defaultPort: number | null;
  /** False for roadmap engines — they render disabled in the picker. */
  implemented: boolean;
  /** Roadmap note appended to the label of disabled engines. */
  roadmap?: string;
  /** Host input placeholder when no preset is active. */
  hostPlaceholder?: string;
  /** Database input placeholder when no preset is active. */
  databasePlaceholder?: string;
  /** Short hint lines rendered under the form. */
  hints: string[];
};

/**
 * Keyed by engine so the compiler enforces one entry per `Engine`; the object
 * insertion order is also the picker's display order.
 */
export const ENGINE_META: Record<Engine, EngineMeta> = {
  postgres: {
    engine: "postgres",
    label: "PostgreSQL",
    group: "SQL",
    kind: "sql",
    defaultPort: 5432,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    hints: [],
  },
  mysql: {
    engine: "mysql",
    label: "MySQL",
    group: "SQL",
    kind: "sql",
    defaultPort: 3306,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    hints: [],
  },
  mariadb: {
    engine: "mariadb",
    label: "MariaDB",
    group: "SQL",
    kind: "sql",
    defaultPort: 3306,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    hints: [],
  },
  sql_server: {
    engine: "sql_server",
    label: "SQL Server",
    group: "SQL",
    kind: "sql",
    defaultPort: 1433,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    hints: [
      "SQL authentication only (Windows / Azure AD logins are not supported).",
      "Named instances: enter the instance's TCP port — SQL Browser discovery is not used.",
    ],
  },
  sqlite: {
    engine: "sqlite",
    label: "SQLite (file)",
    group: "SQL",
    kind: "sql",
    defaultPort: null,
    implemented: true,
    hints: [],
  },
  oracle: {
    engine: "oracle",
    label: "Oracle",
    group: "SQL",
    kind: "sql",
    defaultPort: 1521,
    implemented: false,
    roadmap: "roadmap P3",
    hints: [],
  },
  snowflake: {
    engine: "snowflake",
    label: "Snowflake",
    group: "SQL",
    kind: "sql",
    defaultPort: 443,
    implemented: false,
    roadmap: "roadmap P4",
    hints: [],
  },
  mongodb: {
    engine: "mongodb",
    label: "MongoDB",
    group: "NoSQL",
    kind: "document",
    defaultPort: 27017,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    hints: [
      "Query: db.<collection>.find({…}).limit(n) · aggregate · countDocuments.",
      "Params: auth_source (e.g. admin), replica_set, tls_insecure (DocumentDB global bundle), direct, app_name.",
    ],
  },
  redis: {
    engine: "redis",
    label: "Redis",
    group: "NoSQL",
    kind: "key_value",
    defaultPort: 6379,
    implemented: true,
    hostPlaceholder: "127.0.0.1",
    databasePlaceholder: "DB index (0–15)",
    hints: [
      "Database is the Redis logical DB index (0–15, default 0).",
      "Commands run as-is; PREVIEW <key> opens a key in the grid.",
    ],
  },
  kafka: {
    engine: "kafka",
    label: "Kafka",
    group: "Message broker",
    kind: "message_broker",
    defaultPort: 9092,
    implemented: true,
    hostPlaceholder: "broker-1.example.com, broker-2.example.com",
    databasePlaceholder: "cluster (optional)",
    hints: [
      "Host accepts a comma-separated broker list.",
      "SASL: username = provider API key, password = secret; params sasl_mechanism = plain | scram-sha-256 | scram-sha-512.",
      'Query JSON: {"topic": "…", "from": "latest", "limit": 100}.',
    ],
  },
};

/** Metadata in picker display order (object insertion order). */
export const ENGINES: readonly EngineMeta[] = Object.values(ENGINE_META);

export function enginesInGroup(group: EngineGroup): readonly EngineMeta[] {
  return ENGINES.filter((meta) => meta.group === group);
}

// ---------------------------------------------------------------------------
// Cloud presets
// ---------------------------------------------------------------------------

export type PresetDefaults = {
  port?: number;
  ssl_mode?: SslMode;
  params?: Record<string, string>;
  /** Host input placeholder while the preset is active. */
  hostPlaceholder?: string;
  /** Database input placeholder while the preset is active. */
  databaseHint?: string;
};

export type EnginePreset = {
  id: string;
  label: string;
  engine: Engine;
  /** Descriptive cloud flavor stored on the profile (`ProfileInput.variant`). */
  variant: string;
  defaults: PresetDefaults;
};

export const PRESETS: readonly EnginePreset[] = [
  {
    id: "aurora-mysql",
    label: "AWS Aurora (MySQL)",
    engine: "mysql",
    variant: "aurora",
    defaults: {
      port: 3306,
      ssl_mode: "require",
      hostPlaceholder: "db.cluster-xxx.us-east-1.rds.amazonaws.com",
      databaseHint: "database name (optional)",
    },
  },
  {
    id: "aurora-postgres",
    label: "AWS Aurora (PostgreSQL)",
    engine: "postgres",
    variant: "aurora",
    defaults: {
      port: 5432,
      ssl_mode: "require",
      hostPlaceholder: "db.cluster-xxx.us-east-1.rds.amazonaws.com",
      databaseHint: "database name (e.g. postgres)",
    },
  },
  {
    id: "documentdb",
    label: "AWS DocumentDB",
    engine: "mongodb",
    variant: "documentdb",
    defaults: {
      port: 27017,
      ssl_mode: "require",
      params: { tls_insecure: "true" },
      hostPlaceholder: "xxx.docdb.amazonaws.com",
      databaseHint: "database name (optional)",
    },
  },
  {
    id: "redshift",
    label: "Amazon Redshift",
    engine: "postgres",
    variant: "redshift",
    defaults: {
      port: 5439,
      ssl_mode: "require",
      hostPlaceholder: "redshift-cluster-1.abc123.us-east-1.redshift.amazonaws.com",
      databaseHint: "database name (e.g. dev)",
    },
  },
  {
    id: "azure-sql",
    label: "Azure SQL",
    engine: "sql_server",
    variant: "azure",
    defaults: {
      port: 1433,
      ssl_mode: "require",
      hostPlaceholder: "xxx.database.windows.net",
      databaseHint: "database name",
    },
  },
  {
    id: "elasticache",
    label: "Amazon ElastiCache",
    engine: "redis",
    variant: "elasticache",
    defaults: {
      port: 6379,
      ssl_mode: "require",
      hostPlaceholder: "cluster-name.xxxxxx.use1.cache.amazonaws.com",
      databaseHint: "DB index (0–15)",
    },
  },
  {
    id: "memorystore",
    label: "Google Memorystore",
    engine: "redis",
    variant: "memorystore",
    defaults: {
      port: 6379,
      ssl_mode: "require",
      hostPlaceholder: "10.x.x.x (private IP)",
      databaseHint: "DB index (0–15)",
    },
  },
  {
    id: "confluent-cloud",
    label: "Confluent Cloud",
    engine: "kafka",
    variant: "confluent",
    defaults: {
      port: 9092,
      ssl_mode: "require",
      params: { sasl_mechanism: "plain" },
      hostPlaceholder: "pkc-xxxxx.us-east-1.aws.confluent.cloud",
      databaseHint: "cluster (optional)",
    },
  },
  {
    id: "aiven-kafka",
    label: "Aiven for Apache Kafka",
    engine: "kafka",
    variant: "aiven",
    defaults: {
      // Aiven assigns per-service ports; keep the engine default (9092).
      ssl_mode: "require",
      params: { sasl_mechanism: "scram-sha-256" },
      hostPlaceholder: "kafka-xxxxx-project.aivencloud.com",
      databaseHint: "cluster (optional)",
    },
  },
];

export function presetsForEngine(engine: Engine): readonly EnginePreset[] {
  return PRESETS.filter((preset) => preset.engine === engine);
}

export function presetFor(
  engine: Engine,
  variant: string | null | undefined,
): EnginePreset | null {
  if (!variant) return null;
  return (
    PRESETS.find(
      (preset) => preset.engine === engine && preset.variant === variant,
    ) ?? null
  );
}
