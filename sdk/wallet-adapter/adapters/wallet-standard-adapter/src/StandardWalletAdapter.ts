// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { SignableTransaction } from "@mysten/sui.js";
import {
  WalletAdapter,
  WalletAdapterEvents,
} from "@mysten/wallet-adapter-base";
import { StandardWalletAdapterWallet } from "@mysten/wallet-standard";
import mitt from "mitt";

export interface StandardWalletAdapterConfig {
  wallet: StandardWalletAdapterWallet;
}

type WalletAdapterEventsMap = {
  [E in keyof WalletAdapterEvents]: Parameters<WalletAdapterEvents[E]>[0];
};

export class StandardWalletAdapter implements WalletAdapter {
  connected = false;
  connecting = false;

  #wallet: StandardWalletAdapterWallet;
  readonly #events = mitt<WalletAdapterEventsMap>();

  constructor({ wallet }: StandardWalletAdapterConfig) {
    console.log("StandardWalletAdapter construct");
    this.#wallet = wallet;
    this.#wallet.features["standard:events"].on("change", ({ accounts }) => {
      if (accounts) {
        this.connected = accounts.length > 0;
        this.#events.emit("changed", {
          connected: this.connected,
          accounts: accounts.map((anAccount) => anAccount.address),
        });
      }
    });
  }

  get name() {
    return this.#wallet.name;
  }

  get icon() {
    return this.#wallet.icon;
  }

  get wallet() {
    return this.#wallet;
  }

  async getAccounts() {
    return this.#wallet.accounts.map((account) => account.address);
  }

  async connect() {
    try {
      if (this.connected || this.connecting) return;
      this.connecting = true;

      if (!this.#wallet.accounts.length) {
        await this.#wallet.features["standard:connect"].connect();
      }

      if (!this.#wallet.accounts.length) {
        throw new Error("No wallet accounts found");
      }

      this.connected = true;
    } finally {
      this.connecting = false;
    }
  }

  async disconnect() {
    this.connected = false;
    this.connecting = false;
    if (this.#wallet.features["standard:disconnect"]) {
      await this.#wallet.features["standard:disconnect"].disconnect();
    }
  }

  async signAndExecuteTransaction(transaction: SignableTransaction) {
    return this.#wallet.features[
      "sui:signAndExecuteTransaction"
    ].signAndExecuteTransaction({
      transaction,
    });
  }

  on: <E extends keyof WalletAdapterEvents>(
    event: E,
    callback: WalletAdapterEvents[E]
  ) => () => void = (event, callback) => {
    this.#events.on(event, callback);
    return () => {
      this.#events.off(event, callback);
    };
  };
}
