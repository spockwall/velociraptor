type Variant = "green" | "red" | "gray";

interface Props {
    label: string;
    variant?: Variant;
}

export default function Badge({ label, variant = "gray" }: Props) {
    const variantClasses = {
        green: "bg-accent-green/15 text-accent-green border-accent-green/20",
        red: "bg-accent-red/15 text-accent-red border-accent-red/20",
        gray: "bg-bg-secondary text-text-muted border-border-strong",
    };

    return (
        <span
            className={`inline-flex items-center px-2 py-0.5 rounded-md border text-[11px] font-mono tracking-wide ${variantClasses[variant]}`}
        >
            {label}
        </span>
    );
}
