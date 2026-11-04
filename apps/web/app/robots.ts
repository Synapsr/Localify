import type { MetadataRoute } from "next";

import { siteUrl } from "@/app/site-metadata";

const robots = (): MetadataRoute.Robots => ({
  rules: [
    {
      userAgent: "*",
      allow: "/",
    },
  ],
  sitemap: `${siteUrl}/sitemap.xml`,
});

export default robots;

