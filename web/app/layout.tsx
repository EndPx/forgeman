import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "ForgeMan — Engineering Runs",
  description:
    "Autonomous Software Engineering Agent — evidence that the solution works.",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <div className="wrap">
          <header className="top">
            <div className="brand">
              FORGE<span>MAN</span>
            </div>
            <div className="tagline">AI that engineers, not just codes.</div>
          </header>
          {children}
        </div>
      </body>
    </html>
  );
}
