create table if not exists task_completions (
  id uuid primary key,
  campaign_id uuid not null references campaigns(id) on delete cascade,
  user_id uuid not null references users(id) on delete cascade,
  task_name text not null,
  details text,
  created_at timestamptz not null default now()
);

create index if not exists task_completions_campaign_id_idx
  on task_completions(campaign_id);

create index if not exists task_completions_user_id_idx
  on task_completions(user_id);

create index if not exists task_completions_campaign_user_task_idx
  on task_completions(campaign_id, user_id, task_name);
