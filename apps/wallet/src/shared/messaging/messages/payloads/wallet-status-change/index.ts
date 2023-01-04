// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { isBasePayload } from '_payloads';

import type { SuiAddress } from '@mysten/sui.js/src';
import type { BasePayload, Payload } from '_payloads';
import type { API_ENV } from '_src/ui/app/ApiProvider';

export type WalletStatusChange = {
    network?:
        | Exclude<API_ENV, API_ENV.customRPC>
        | `${'http' | 'https'}://${string}`;
    accounts?: SuiAddress[];
};

export interface WalletStatusChangePayload
    extends BasePayload,
        WalletStatusChange {
    type: 'wallet-status-changed';
}

export function isWalletStatusChangePayload(
    payload: Payload
): payload is WalletStatusChangePayload {
    return isBasePayload(payload) && payload.type === 'wallet-status-changed';
}
