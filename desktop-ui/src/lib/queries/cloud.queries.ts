import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  CloudAccountDto,
  CloudSignInPollDto,
  CloudSignInStartDto,
  CreatePublicUrlRequest,
  PollCloudSignInRequest,
  PublicUrlDto,
  PublicUrlRequest,
  PublicUrlWithPasswordDto,
  SetPublicUrlProtectionRequest,
} from "../../types/localify";

import { tauriClient } from "../tauri-client";
import { invalidateQueries } from "./shared";

const cloudQueryKeys = {
  all: () => ["cloud"] as const,
  account: () => [...cloudQueryKeys.all(), "account"] as const,
  publicUrls: () => [...cloudQueryKeys.all(), "public-urls"] as const,
} as const;

/** The account and the public URL list both live under `all`. */
const cloudMutationInvalidations = [cloudQueryKeys.all()] as const;

export const cloudQueries = {
  all: cloudQueryKeys.all,
  account: () =>
    queryOptions<CloudAccountDto, Error>({
      queryKey: cloudQueryKeys.account(),
      queryFn: () => tauriClient.getCloudAccount(),
      refetchInterval: (query) => (query.state.data?.tunnel.state === "starting" ? 3000 : false),
    }),
  /** Calls Localify Cloud: only while signed in. */
  publicUrls: () =>
    queryOptions<PublicUrlDto[], Error>({
      queryKey: cloudQueryKeys.publicUrls(),
      queryFn: () => tauriClient.listPublicUrls(),
      staleTime: 30_000,
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
    mutationOptions<CloudAccountDto, Error, void>({
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
