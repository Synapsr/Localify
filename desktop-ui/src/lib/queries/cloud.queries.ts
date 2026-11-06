import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  CloudAccountDto,
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
} from "../../types/localify";

import { tauriClient } from "../tauri-client";
import { invalidateQueries } from "./shared";

const cloudQueryKeys = {
  all: () => ["cloud"] as const,
  account: () => [...cloudQueryKeys.all(), "account"] as const,
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

export const cloudMutations = {
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
