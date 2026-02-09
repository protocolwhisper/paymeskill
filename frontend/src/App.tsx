import { FormEvent, useEffect, useMemo, useState } from "react";

type Campaign = {
  id: string;
  name: string;
  sponsor: string;
  target_roles: string[];
  target_tools: string[];
  required_task: string;
  subsidy_per_call_cents: number;
  budget_remaining_cents: number;
  active: boolean;
  created_at: string;
};

type Profile = {
  id: string;
  email: string;
  region: string;
  roles: string[];
  tools_used: string[];
  created_at: string;
};

type CreatorSummary = {
  total_events: number;
  success_events: number;
  success_rate: number;
  per_skill: Array<{
    skill_name: string;
    total_events: number;
    success_events: number;
    avg_duration_ms: number | null;
    last_seen_at: string;
  }>;
};

type CampaignForm = {
  name: string;
  sponsor: string;
  target_roles: string;
  target_tools: string;
  required_task: string;
  subsidy_per_call_cents: number;
  budget_cents: number;
};

const defaultCampaignForm: CampaignForm = {
  name: "",
  sponsor: "",
  target_roles: "developer",
  target_tools: "scraping",
  required_task: "signup_sponsor",
  subsidy_per_call_cents: 5,
  budget_cents: 500
};

function App() {
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [creator, setCreator] = useState<CreatorSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [createLoading, setCreateLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<CampaignForm>(defaultCampaignForm);

  async function fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`/api${path}`, {
      ...init,
      headers: {
        "content-type": "application/json",
        ...(init?.headers ?? {})
      }
    });

    if (!response.ok) {
      const message = await response.text();
      throw new Error(message || `Request failed (${response.status})`);
    }

    return response.json() as Promise<T>;
  }

  async function loadDashboard() {
    setLoading(true);
    setError(null);

    try {
      const [campaignData, profileData, creatorData] = await Promise.all([
        fetchJson<Campaign[]>("/campaigns", { method: "GET" }),
        fetchJson<Profile[]>("/profiles", { method: "GET" }),
        fetchJson<CreatorSummary>("/creator/metrics", { method: "GET" })
      ]);
      setCampaigns(campaignData);
      setProfiles(profileData);
      setCreator(creatorData);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void loadDashboard();
  }, []);

  const totals = useMemo(() => {
    const activeCampaigns = campaigns.filter((item) => item.active).length;
    const subsidyCents = campaigns.reduce(
      (acc, item) => acc + item.subsidy_per_call_cents,
      0
    );
    const budgetCents = campaigns.reduce(
      (acc, item) => acc + item.budget_remaining_cents,
      0
    );

    return {
      activeCampaigns,
      subsidyDollars: (subsidyCents / 100).toFixed(2),
      budgetDollars: (budgetCents / 100).toFixed(2),
      profiles: profiles.length,
      creatorSuccessRate: ((creator?.success_rate ?? 0) * 100).toFixed(1)
    };
  }, [campaigns, profiles.length, creator?.success_rate]);

  async function onCreateCampaign(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setCreateLoading(true);
    setError(null);

    try {
      await fetchJson<Campaign>("/campaigns", {
        method: "POST",
        body: JSON.stringify({
          ...form,
          target_roles: splitCsv(form.target_roles),
          target_tools: splitCsv(form.target_tools)
        })
      });
      setForm(defaultCampaignForm);
      await loadDashboard();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setCreateLoading(false);
    }
  }

  return (
    <div className="page">
      <header className="topbar glass">
        <div className="brand">
          <span className="brand-mark" aria-hidden>
            PX
          </span>
          <div>
            <p className="brand-title">payloadex</p>
            <p className="brand-sub">Sponsored Compute Exchange</p>
          </div>
        </div>
        <div className="top-actions">
          <button className="ghost">All Chains</button>
          <button className="ghost">Search</button>
        </div>
      </header>

      <main className="content">
        <section className="hero">
          <p className="eyebrow">Overview</p>
          <h1>Agent payouts, sponsor campaigns, and x402 usage telemetry</h1>
          <p>
            This is your operator surface. Sponsors fund campaigns, agents route
            requests through paid endpoints, and Payloadex tracks settlement and
            usage quality.
          </p>
        </section>

        <section className="kpi-grid">
          <StatCard label="Active Campaigns" value={String(totals.activeCampaigns)} />
          <StatCard label="Remaining Budget" value={`$${totals.budgetDollars}`} />
          <StatCard label="Avg Subsidy / Call" value={`$${totals.subsidyDollars}`} />
          <StatCard label="Registered Builders" value={String(totals.profiles)} />
          <StatCard
            label="Skill Success Rate"
            value={`${totals.creatorSuccessRate}%`}
          />
        </section>

        {error ? <p className="error">{error}</p> : null}

        <section className="panel-row">
          <article className="panel glass">
            <div className="panel-head">
              <h2>Sponsor Campaigns</h2>
              <button className="ghost" onClick={() => void loadDashboard()}>
                Refresh
              </button>
            </div>
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Sponsor</th>
                    <th>Targets</th>
                    <th>Subsidy</th>
                    <th>Budget Left</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {loading ? (
                    <tr>
                      <td colSpan={6}>Loading dashboard...</td>
                    </tr>
                  ) : campaigns.length === 0 ? (
                    <tr>
                      <td colSpan={6}>No campaigns yet.</td>
                    </tr>
                  ) : (
                    campaigns.map((campaign) => (
                      <tr key={campaign.id}>
                        <td>{campaign.name}</td>
                        <td>{campaign.sponsor}</td>
                        <td>
                          {campaign.target_roles.join(", ")} / {campaign.target_tools.join(", ")}
                        </td>
                        <td>${(campaign.subsidy_per_call_cents / 100).toFixed(2)}</td>
                        <td>${(campaign.budget_remaining_cents / 100).toFixed(2)}</td>
                        <td>
                          <span className={campaign.active ? "pill active" : "pill paused"}>
                            {campaign.active ? "ACTIVE" : "PAUSED"}
                          </span>
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </article>

          <article className="panel glass">
            <div className="panel-head">
              <h2>Create Campaign</h2>
              <p>Launch a payout stream for target developer segments.</p>
            </div>
            <form className="campaign-form" onSubmit={onCreateCampaign}>
              <label>
                Campaign Name
                <input
                  required
                  value={form.name}
                  onChange={(event) =>
                    setForm((prev) => ({ ...prev, name: event.target.value }))
                  }
                />
              </label>
              <label>
                Sponsor
                <input
                  required
                  value={form.sponsor}
                  onChange={(event) =>
                    setForm((prev) => ({ ...prev, sponsor: event.target.value }))
                  }
                />
              </label>
              <label>
                Target Roles (comma-separated)
                <input
                  value={form.target_roles}
                  onChange={(event) =>
                    setForm((prev) => ({ ...prev, target_roles: event.target.value }))
                  }
                />
              </label>
              <label>
                Target Tools (comma-separated)
                <input
                  value={form.target_tools}
                  onChange={(event) =>
                    setForm((prev) => ({ ...prev, target_tools: event.target.value }))
                  }
                />
              </label>
              <label>
                Required Task
                <input
                  required
                  value={form.required_task}
                  onChange={(event) =>
                    setForm((prev) => ({ ...prev, required_task: event.target.value }))
                  }
                />
              </label>
              <div className="form-split">
                <label>
                  Subsidy / Call (cents)
                  <input
                    required
                    min={1}
                    type="number"
                    value={form.subsidy_per_call_cents}
                    onChange={(event) =>
                      setForm((prev) => ({
                        ...prev,
                        subsidy_per_call_cents: Number(event.target.value)
                      }))
                    }
                  />
                </label>
                <label>
                  Budget (cents)
                  <input
                    required
                    min={1}
                    type="number"
                    value={form.budget_cents}
                    onChange={(event) =>
                      setForm((prev) => ({
                        ...prev,
                        budget_cents: Number(event.target.value)
                      }))
                    }
                  />
                </label>
              </div>
              <button className="primary" disabled={createLoading}>
                {createLoading ? "Creating..." : "Create Campaign"}
              </button>
            </form>
          </article>
        </section>

        <section className="panel-row two-col">
          <article className="panel glass">
            <div className="panel-head">
              <h2>Skill Telemetry</h2>
            </div>
            <p className="metric-large">{creator?.total_events ?? 0}</p>
            <p className="metric-label">Total creator events</p>
            <div className="skill-list">
              {(creator?.per_skill ?? []).slice(0, 5).map((skill) => (
                <div key={skill.skill_name} className="skill-item">
                  <strong>{skill.skill_name}</strong>
                  <span>
                    {skill.success_events}/{skill.total_events} successful
                  </span>
                </div>
              ))}
            </div>
          </article>

          <article className="panel glass">
            <div className="panel-head">
              <h2>Integration Path</h2>
            </div>
            <ol className="flow-list">
              <li>Agent calls your paid `scrape_url` tool.</li>
              <li>Tool bridge forwards to `/proxy/scraping/run`.</li>
              <li>If no sponsor match, bridge handles `402` payment path.</li>
              <li>After settlement, payload is returned to caller.</li>
            </ol>
          </article>
        </section>
      </main>
    </div>
  );
}

function splitCsv(raw: string): string[] {
  return raw
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
}

function StatCard(props: { label: string; value: string }) {
  return (
    <article className="stat-card glass">
      <p>{props.label}</p>
      <h3>{props.value}</h3>
      <div className="sparkline" aria-hidden>
        <span />
        <span />
        <span />
        <span />
        <span />
        <span />
      </div>
    </article>
  );
}

export default App;
