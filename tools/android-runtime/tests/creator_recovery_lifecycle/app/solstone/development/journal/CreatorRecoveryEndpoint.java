// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

package app.solstone.development.journal;
/** Framework/native adapter double: lifecycle test only. Actual adapter is separately SDK-compiled. */
final class CreatorRecoveryEndpoint {static String command(CreatorRecoveryState.Request r,long epoch){return r.action.name();}}
