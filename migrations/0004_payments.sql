create table if not exists payments (
  tx_hash text primary key,
  campaign_id uuid references campaigns(id) on delete set null,
  service text not null,
  amount_cents bigint not null,
  payer text not null,
  source text not null check (source in ('user', 'sponsor')),
  status text not null check (status in ('settled', 'failed')),
  created_at timestamptz not null default now()
);

create index if not exists payments_campaign_id_idx
  on payments(campaign_id);

create index if not exists payments_source_status_idx
  on payments(source, status);

create index if not exists payments_created_at_idx
  on payments(created_at desc);
