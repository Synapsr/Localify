import type { NextConfig } from "next";
import { initOpenNextCloudflareForDev } from "@opennextjs/cloudflare";
import createNextIntlPlugin from "next-intl/plugin";

const nextConfig: NextConfig = {
  allowedDevOrigins: [
    "localify.localify",
    "web-localify.localify",
    "localify-update.localify",
  ],
  images: {
    unoptimized: true,
  },
  reactStrictMode: true,
};

const withNextIntl = createNextIntlPlugin("./i18n/request.ts");

export default withNextIntl(nextConfig);

initOpenNextCloudflareForDev();
