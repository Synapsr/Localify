import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  CloudAccountDto,
  CloudBillingPageDto,
  CloudPlanDto,
  CloudSignInPollDto,
  CloudSignInStartDto,
  CloudSignOutDto,
  CreatePublicUrlRequest,
  PollCloudSignInRequest,
  PublicUrlDto,
  PublicUrlRequest,
  PublicUrlSlugCheckDto,
  PublicUrlSlugSuggestionDto,
  PublicUrlWithPasswordDto,
  SetPublicUrlProtectionRequest,
  StartCloudCheckoutRequest,
} from "../../types/localify";

import { createEntitlementTracker } from "../plan-entitlement";
import { tauriClient } from "../tauri-client";
import { invalidateQueries } from "./shared";

/** After a checkout or the portal opens, the plan is read this often, for this long. */
const CHECKOUT_POLL_MS = 5000;
const CHECKOUT_WATCH_MS = 15 * 60_000;

/** Until when the app polls the plan for a checkout or portal it opened. */
let checkoutWatchUntil = 0;

function isWatchingCheckout(): boolean {
  return Date.now() < checkoutWatchUntil;
}

/**
 * Whether the plan's entitlement changed since the last answer any screen saw, however it
 * changed. The checkout watch only makes the app ask more often.
 */
export const entitlementChange = createEntitlementTracker();

const cloudQueryKeys = {
  all: () => ["cloud"] as const,
  account: () => [...cloudQueryKeys.all(), "account"] as const,
  plan: () => [...cloudQueryKeys.all(), "plan"] as const,
  publicUrls: () => [...cloudQueryKeys.all(), "public-urls"] as const,
  slugSuggestion: (localDomain: string) =>
    [...cloudQueryKeys.all(), "slug-suggestion", localDomain] as const,
  slugCheck: (slug: string) => [...cloudQueryKeys.all(), "slug-check", slug] as const,
} as const;

/** The account and the public URL list both live under `all`. */
const cloudMutationInvalidations = [cloudQueryKeys.all()] as const;

export const cloudQueries = {
  all: cloudQueryKeys.all,
  account: () =>
    queryOptions<CloudAccountDto, Error>({
      queryKey: cloudQueryKeys.account(),
      queryFn: () => tauriClient.getCloudAccount(),
      // The daemon retries the tunnel by itself and starts it after the first
      // URL, so keep looking while signed in, faster while it starts.
      refetchInterval: (query) => {
        const account = query.state.data;
        if (!account?.signedIn) return false;
        return account.tunnel.state === "starting" ? 3000 : 10_000;
      },
    }),
  /**
   * Calls Localify Cloud: only while signed in. Null from a cloud without plans. Polled while
   * a checkout or the portal is open, and on every focus of the window then.
   */
  plan: () =>
    queryOptions<CloudPlanDto | null, Error>({
      queryKey: cloudQueryKeys.plan(),
      queryFn: () => tauriClient.getCloudPlan(),
      staleTime: 30_000,
      refetchInterval: () => (isWatchingCheckout() ? CHECKOUT_POLL_MS : false),
      refetchOnWindowFocus: () => (isWatchingCheckout() ? "always" : true),
    }),
  /** Calls Localify Cloud: only while signed in. */
  publicUrls: () =>
    queryOptions<PublicUrlDto[], Error>({
      queryKey: cloudQueryKeys.publicUrls(),
      queryFn: () => tauriClient.listPublicUrls(),
      staleTime: 30_000,
    }),
  /** The name a new URL for `localDomain` gets by default. Calls Localify Cloud. */
  slugSuggestion: (localDomain: string) =>
    queryOptions<PublicUrlSlugSuggestionDto, Error>({
      queryKey: cloudQueryKeys.slugSuggestion(localDomain),
      queryFn: () => tauriClient.suggestPublicUrlSlug({ localDomain }),
      staleTime: 30_000,
    }),
  /** Whether a well-formed slug is free. Calls Localify Cloud. */
  slugCheck: (slug: string) =>
    queryOptions<PublicUrlSlugCheckDto, Error>({
      queryKey: cloudQueryKeys.slugCheck(slug),
      queryFn: () => tauriClient.checkPublicUrlSlug({ slug }),
      staleTime: 10_000,
      retry: false,
    }),
} as const;

/** Starts waiting on the page that just opened in the browser. */
async function watchCheckout(queryClient: QueryClient) {
  checkoutWatchUntil = Date.now() + CHECKOUT_WATCH_MS;
  await invalidateQueries(queryClient, [cloudQueryKeys.plan()]);
}

export const cloudMutations = {
  /** Opens the Polar checkout (or the portal when the plan is active) in the browser. */
  startCheckout: (queryClient: QueryClient) =>
    mutationOptions<CloudBillingPageDto, Error, StartCloudCheckoutRequest>({
      mutationFn: (request) => tauriClient.startCloudCheckout(request),
      onSuccess: async () => watchCheckout(queryClient),
    }),
  openBillingPortal: (queryClient: QueryClient) =>
    mutationOptions<void, Error, void>({
      mutationFn: () => tauriClient.openCloudBillingPortal(),
      onSuccess: async () => watchCheckout(queryClient),
    }),
  /**
   * After the plan's entitlement changed: the daemon fetches what this Mac serves, and the
   * account, the plan and the URLs, whose state follows the plan, are read again.
   */
  refreshTunnel: (queryClient: QueryClient) =>
    mutationOptions<void, Error, void>({
      mutationFn: () => tauriClient.refreshPublicTunnel(),
      onSettled: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
  startSignIn: () =>
    mutationOptions<CloudSignInStartDto, Error, void>({
      mutationFn: () => tauriClient.startCloudSignIn(),
    }),
  pollSignIn: (queryClient: QueryClient) =>
    mutationOptions<CloudSignInPollDto, Error, PollCloudSignInRequest>({
      mutationFn: (request) => tauriClient.pollCloudSignIn(request),
      onSuccess: async (result) => {
        if (result.status === "signedIn") {
          await invalidateQueries(queryClient, cloudMutationInvalidations);
        }
      },
    }),
  signOut: (queryClient: QueryClient) =>
    mutationOptions<CloudSignOutDto, Error, void>({
      mutationFn: () => tauriClient.signOutCloud(),
      onSuccess: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
  createPublicUrl: (queryClient: QueryClient) =>
    mutationOptions<PublicUrlWithPasswordDto, Error, CreatePublicUrlRequest>({
      mutationFn: (request) => tauriClient.createPublicUrl(request),
      onSuccess: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
  removePublicUrl: (queryClient: QueryClient) =>
    mutationOptions<void, Error, PublicUrlRequest>({
      mutationFn: (request) => tauriClient.removePublicUrl(request),
      onSuccess: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
  setProtection: (queryClient: QueryClient) =>
    mutationOptions<PublicUrlWithPasswordDto, Error, SetPublicUrlProtectionRequest>({
      mutationFn: (request) => tauriClient.setPublicUrlProtection(request),
      onSuccess: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
  claimPublicUrl: (queryClient: QueryClient) =>
    mutationOptions<PublicUrlDto, Error, PublicUrlRequest>({
      mutationFn: (request) => tauriClient.claimPublicUrl(request),
      onSuccess: async () => invalidateQueries(queryClient, cloudMutationInvalidations),
    }),
} as const;
