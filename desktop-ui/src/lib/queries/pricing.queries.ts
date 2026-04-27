import { queryOptions } from "@tanstack/react-query";
import { z } from "zod";

const localizedPricingSchema = z.object({
  currency: z.enum(["eur", "gbp", "usd"]),
  freeLabel: z.string(),
  plans: z.object({
    single: z.object({
      currency: z.enum(["eur", "gbp", "usd"]),
      amountCents: z.number(),
      label: z.string(),
    }),
    personal: z.object({
      currency: z.enum(["eur", "gbp", "usd"]),
      amountCents: z.number(),
      label: z.string(),
    }),
  }),
});

export type LocalizedPricingDto = z.infer<typeof localizedPricingSchema>;

const fetchLocalizedPricing = async () => {
  const response = await fetch("https://localify.dev/api/pricing");

  if (!response.ok) {
    throw new Error("Failed to fetch Localify pricing.");
  }

  return localizedPricingSchema.parse(await response.json());
};

export const pricingQueries = {
  all: () => ["pricing"] as const,
  localized: () =>
    queryOptions<LocalizedPricingDto, Error>({
      queryKey: [...pricingQueries.all(), "localized"],
      queryFn: fetchLocalizedPricing,
      staleTime: 60 * 60 * 1000,
      retry: 1,
    }),
} as const;
