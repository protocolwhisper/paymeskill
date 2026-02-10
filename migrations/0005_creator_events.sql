create table if not exists creator_events (
  id uuid primary key,
  skill_name text not null,
  platform text not null,
  event_type text not null,
  duration_ms bigint,
  success boolean not null,
  created_at timestamptz not null default now()
);

create index if not exists creator_events_skill_platform_idx
  on creator_events(skill_name, platform);

create index if not exists creator_events_created_at_idx
  on creator_events(created_at desc);
