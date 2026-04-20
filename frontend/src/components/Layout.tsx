import { NavLink, Outlet } from "react-router-dom";
import Logo from "./Logo";

const nav = [
    { to: "/orderbook", label: "Orderbook" },
    { to: "/trades", label: "Trades" },
    { to: "/account", label: "Account" },
    { to: "/control", label: "Control" },
];

export default function Layout() {
    return (
        <div className="flex flex-col min-h-screen bg-bg-base text-text-primary">
            {/* Horizontal navbar */}
            <header className="flex items-center gap-8 px-6 py-3 border-b border-border-strong bg-bg-surface flex-shrink-0 shadow-sm transition-all">
                {/* Logo */}
                <NavLink to="/" className="flex items-center text-text-primary hover:text-white transition-colors">
                    <Logo className="h-6 w-auto" />
                </NavLink>

                {/* Nav links */}
                <nav className="flex items-center gap-2">
                    {nav.map(({ to, label }) => (
                        <NavLink
                            key={to}
                            to={to}
                            className={({ isActive }) =>
                                `px-3 py-1.5 rounded-md text-sm font-medium transition-all duration-200 border ${
                                    isActive
                                        ? "text-white bg-bg-hover border-border-strong shadow-sm"
                                        : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover hover:border-border-subtle"
                                }`
                            }
                        >
                            {label}
                        </NavLink>
                    ))}
                </nav>
            </header>

            {/* Page content */}
            <main className="flex-1 w-full max-w-[1600px] mx-auto overflow-y-auto flex flex-col">
                <Outlet />
            </main>
        </div>
    );
}
