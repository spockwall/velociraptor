import { useCallback, useMemo, useState } from "react";
import Card from "../../components/Card";
import { api } from "../../lib/api";
import type { UserEvent } from "../../lib/api";
import { usePolling } from "../../lib/usePolling";
import { AccountTabs, FETCH_LIMIT, OrdersTable, POLL_INTERVAL_MS, SearchBox, eventMatchesSearch } from "./shared";

export default function OrdersPage() {
    const [search, setSearch] = useState("");
    const fetcher = useCallback(() => api.orders(FETCH_LIMIT), []);
    const { data, loading, error } = usePolling<UserEvent[]>(fetcher, POLL_INTERVAL_MS);

    const filtered = useMemo(() => (data ?? []).filter((e) => eventMatchesSearch(e, search)), [data, search]);

    return (
        <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto">
            <AccountTabs />
            <SearchBox value={search} onChange={setSearch} />
            <Card title="order updates" subtitle="events:orders (Redis)">
                <OrdersTable events={filtered} loading={loading} error={error} />
            </Card>
        </div>
    );
}
