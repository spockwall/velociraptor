import { useEffect } from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Layout from "./components/Layout";
import Landing from "./pages/Landing";
import Orderbook from "./pages/Orderbook";
import Polymarket from "./pages/Polymarket";
import Kalshi from "./pages/Kalshi";
import Trades from "./pages/Trades";
import Account from "./pages/Account";
import Control from "./pages/Control";
import themeConfig from "./theme.json";

export default function App() {
    useEffect(() => {
        const root = document.documentElement;
        Object.entries(themeConfig.colors).forEach(([key, value]) => {
            root.style.setProperty(`--theme-${key}`, value);
        });
    }, []);

    return (
        <BrowserRouter>
            <Routes>
                {/* App shell — unified sidebar/navbar layout */}
                <Route element={<Layout />}>
                    <Route path="/" element={<Landing />} />
                    <Route path="/orderbook" element={<Orderbook />} />
                    <Route path="/polymarket" element={<Polymarket />} />
                    <Route path="/kalshi" element={<Kalshi />} />
                    <Route path="/trades" element={<Trades />} />
                    <Route path="/account" element={<Account />} />
                    <Route path="/control" element={<Control />} />
                </Route>
            </Routes>
        </BrowserRouter>
    );
}
