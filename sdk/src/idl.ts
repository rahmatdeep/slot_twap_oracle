export type SlotTwapOracle = {
  address: "7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT";
  metadata: {
    name: "slotTwapOracle";
    version: "0.1.0";
    spec: "0.1.0";
    description: "Created with Anchor";
  };
  instructions: [
    {
      name: "getSwap";
      discriminator: [228, 117, 165, 168, 20, 75, 117, 0];
      accounts: [
        {
          name: "oracle";
          pda: {
            seeds: [
              { kind: "const"; value: [111, 114, 97, 99, 108, 101] },
              { kind: "account"; path: "oracle.base_mint"; account: "Oracle" },
              { kind: "account"; path: "oracle.quote_mint"; account: "Oracle" }
            ];
          };
          relations: ["observationBuffer"];
        },
        {
          name: "observationBuffer";
          pda: {
            seeds: [
              { kind: "const"; value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account"; path: "oracle" }
            ];
          };
        }
      ];
      args: [
        { name: "windowSlots"; type: "u64" },
        { name: "maxStalenessSlots"; type: "u64" }
      ];
      returns: "u128";
    },
    {
      name: "initializeOracle";
      discriminator: [144, 223, 131, 120, 196, 253, 181, 99];
      accounts: [
        {
          name: "oracle";
          writable: true;
          pda: {
            seeds: [
              { kind: "const"; value: [111, 114, 97, 99, 108, 101] },
              { kind: "account"; path: "baseMint" },
              { kind: "account"; path: "quoteMint" }
            ];
          };
        },
        {
          name: "observationBuffer";
          writable: true;
          pda: {
            seeds: [
              { kind: "const"; value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account"; path: "oracle" }
            ];
          };
        },
        { name: "baseMint" },
        { name: "quoteMint" },
        { name: "authority"; writable: true; signer: true },
        { name: "systemProgram"; address: "11111111111111111111111111111111" }
      ];
      args: [{ name: "capacity"; type: "u32" }];
    },
    {
      name: "updatePrice";
      discriminator: [61, 34, 117, 155, 75, 34, 123, 208];
      accounts: [
        { name: "payer"; signer: true },
        { name: "oracle"; writable: true },
        {
          name: "observationBuffer";
          writable: true;
          pda: {
            seeds: [
              { kind: "const"; value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account"; path: "oracle" }
            ];
          };
        }
      ];
      args: [{ name: "newPrice"; type: "u128" }];
    },
    {
      name: "transferOwnership";
      discriminator: [65, 177, 215, 73, 53, 45, 99, 47];
      accounts: [
        { name: "oracle"; writable: true },
        { name: "owner"; signer: true },
        { name: "newOwner" }
      ];
      args: [];
    },
    {
      name: "setPaused";
      discriminator: [91, 60, 125, 192, 176, 225, 166, 218];
      accounts: [
        { name: "oracle"; writable: true },
        { name: "owner"; signer: true }
      ];
      args: [{ name: "paused"; type: "bool" }];
    }
  ];
  accounts: [
    { name: "observationBuffer"; discriminator: [251, 96, 31, 90, 232, 132, 250, 134] },
    { name: "oracle"; discriminator: [139, 194, 131, 179, 140, 179, 229, 244] }
  ];
  events: [
    { name: "oracleUpdate"; discriminator: [237, 176, 133, 150, 0, 131, 48, 15] },
    { name: "ownershipTransferred"; discriminator: [172, 61, 205, 183, 250, 50, 38, 98] },
    { name: "oraclePauseToggled"; discriminator: [75, 169, 119, 212, 242, 216, 255, 126] }
  ];
  errors: [
    { code: 6000; name: "priceOverflow"; msg: "Price overflow detected" },
    { code: 6001; name: "staleSlot"; msg: "Stale oracle update — slot has not advanced" },
    { code: 6002; name: "insufficientHistory"; msg: "Not enough observations to compute swap for requested window" },
    { code: 6003; name: "invalidCapacity"; msg: "Observation buffer capacity must be greater than zero" },
    { code: 6004; name: "staleOracle"; msg: "Oracle data is stale — last update exceeds max staleness threshold" },
    { code: 6005; name: "priceDeviationTooLarge"; msg: "Price deviation from last update exceeds maximum allowed threshold" },
    { code: 6006; name: "unauthorized"; msg: "Signer is not the oracle owner" },
    { code: 6007; name: "oraclePaused"; msg: "Oracle is paused" }
  ];
  types: [
    {
      name: "observation";
      type: {
        kind: "struct";
        fields: [
          { name: "slot"; type: "u64" },
          { name: "cumulativePrice"; type: "u128" }
        ];
      };
    },
    {
      name: "observationBuffer";
      type: {
        kind: "struct";
        fields: [
          { name: "oracle"; type: "pubkey" },
          { name: "head"; type: "u32" },
          { name: "capacity"; type: "u32" },
          { name: "observations"; type: { vec: { defined: { name: "observation" } } } }
        ];
      };
    },
    {
      name: "oracle";
      type: {
        kind: "struct";
        fields: [
          { name: "owner"; type: "pubkey" },
          { name: "baseMint"; type: "pubkey" },
          { name: "quoteMint"; type: "pubkey" },
          { name: "lastPrice"; type: "u128" },
          { name: "cumulativePrice"; type: "u128" },
          { name: "lastSlot"; type: "u64" },
          { name: "lastUpdater"; type: "pubkey" },
          { name: "paused"; type: "bool" }
        ];
      };
    },
    {
      name: "oracleUpdate";
      type: {
        kind: "struct";
        fields: [
          { name: "oracle"; type: "pubkey" },
          { name: "price"; type: "u128" },
          { name: "cumulativePrice"; type: "u128" },
          { name: "slot"; type: "u64" },
          { name: "updater"; type: "pubkey" }
        ];
      };
    },
    {
      name: "ownershipTransferred";
      type: {
        kind: "struct";
        fields: [
          { name: "oracle"; type: "pubkey" },
          { name: "previousOwner"; type: "pubkey" },
          { name: "newOwner"; type: "pubkey" }
        ];
      };
    },
    {
      name: "oraclePauseToggled";
      type: {
        kind: "struct";
        fields: [
          { name: "oracle"; type: "pubkey" },
          { name: "paused"; type: "bool" }
        ];
      };
    }
  ];
};

export const IDL: SlotTwapOracle = {
  address: "7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT",
  metadata: {
    name: "slotTwapOracle",
    version: "0.1.0",
    spec: "0.1.0",
    description: "Created with Anchor",
  },
  instructions: [
    {
      name: "getSwap",
      discriminator: [228, 117, 165, 168, 20, 75, 117, 0],
      accounts: [
        {
          name: "oracle",
          pda: {
            seeds: [
              { kind: "const", value: [111, 114, 97, 99, 108, 101] },
              { kind: "account", path: "oracle.base_mint", account: "Oracle" },
              { kind: "account", path: "oracle.quote_mint", account: "Oracle" },
            ],
          },
          relations: ["observationBuffer"],
        },
        {
          name: "observationBuffer",
          pda: {
            seeds: [
              { kind: "const", value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account", path: "oracle" },
            ],
          },
        },
      ],
      args: [
        { name: "windowSlots", type: "u64" },
        { name: "maxStalenessSlots", type: "u64" },
      ],
      returns: "u128",
    },
    {
      name: "initializeOracle",
      discriminator: [144, 223, 131, 120, 196, 253, 181, 99],
      accounts: [
        {
          name: "oracle",
          writable: true,
          pda: {
            seeds: [
              { kind: "const", value: [111, 114, 97, 99, 108, 101] },
              { kind: "account", path: "baseMint" },
              { kind: "account", path: "quoteMint" },
            ],
          },
        },
        {
          name: "observationBuffer",
          writable: true,
          pda: {
            seeds: [
              { kind: "const", value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account", path: "oracle" },
            ],
          },
        },
        { name: "baseMint" },
        { name: "quoteMint" },
        { name: "authority", writable: true, signer: true },
        { name: "systemProgram", address: "11111111111111111111111111111111" },
      ],
      args: [{ name: "capacity", type: "u32" }],
    },
    {
      name: "updatePrice",
      discriminator: [61, 34, 117, 155, 75, 34, 123, 208],
      accounts: [
        { name: "payer", signer: true },
        { name: "oracle", writable: true },
        {
          name: "observationBuffer",
          writable: true,
          pda: {
            seeds: [
              { kind: "const", value: [111, 98, 115, 101, 114, 118, 97, 116, 105, 111, 110] },
              { kind: "account", path: "oracle" },
            ],
          },
        },
      ],
      args: [{ name: "newPrice", type: "u128" }],
    },
    {
      name: "transferOwnership",
      discriminator: [65, 177, 215, 73, 53, 45, 99, 47],
      accounts: [
        { name: "oracle", writable: true },
        { name: "owner", signer: true },
        { name: "newOwner" },
      ],
      args: [],
    },
    {
      name: "setPaused",
      discriminator: [91, 60, 125, 192, 176, 225, 166, 218],
      accounts: [
        { name: "oracle", writable: true },
        { name: "owner", signer: true },
      ],
      args: [{ name: "paused", type: "bool" }],
    },
  ],
  accounts: [
    { name: "observationBuffer", discriminator: [251, 96, 31, 90, 232, 132, 250, 134] },
    { name: "oracle", discriminator: [139, 194, 131, 179, 140, 179, 229, 244] },
  ],
  events: [
    { name: "oracleUpdate", discriminator: [237, 176, 133, 150, 0, 131, 48, 15] },
    { name: "ownershipTransferred", discriminator: [172, 61, 205, 183, 250, 50, 38, 98] },
    { name: "oraclePauseToggled", discriminator: [75, 169, 119, 212, 242, 216, 255, 126] },
  ],
  errors: [
    { code: 6000, name: "priceOverflow", msg: "Price overflow detected" },
    { code: 6001, name: "staleSlot", msg: "Stale oracle update — slot has not advanced" },
    { code: 6002, name: "insufficientHistory", msg: "Not enough observations to compute swap for requested window" },
    { code: 6003, name: "invalidCapacity", msg: "Observation buffer capacity must be greater than zero" },
    { code: 6004, name: "staleOracle", msg: "Oracle data is stale — last update exceeds max staleness threshold" },
    { code: 6005, name: "priceDeviationTooLarge", msg: "Price deviation from last update exceeds maximum allowed threshold" },
    { code: 6006, name: "unauthorized", msg: "Signer is not the oracle owner" },
    { code: 6007, name: "oraclePaused", msg: "Oracle is paused" },
  ],
  types: [
    {
      name: "observation",
      type: {
        kind: "struct",
        fields: [
          { name: "slot", type: "u64" },
          { name: "cumulativePrice", type: "u128" },
        ],
      },
    },
    {
      name: "observationBuffer",
      type: {
        kind: "struct",
        fields: [
          { name: "oracle", type: "pubkey" },
          { name: "head", type: "u32" },
          { name: "capacity", type: "u32" },
          { name: "observations", type: { vec: { defined: { name: "observation" } } } },
        ],
      },
    },
    {
      name: "oracle",
      type: {
        kind: "struct",
        fields: [
          { name: "owner", type: "pubkey" },
          { name: "baseMint", type: "pubkey" },
          { name: "quoteMint", type: "pubkey" },
          { name: "lastPrice", type: "u128" },
          { name: "cumulativePrice", type: "u128" },
          { name: "lastSlot", type: "u64" },
          { name: "lastUpdater", type: "pubkey" },
          { name: "paused", type: "bool" },
        ],
      },
    },
    {
      name: "oracleUpdate",
      type: {
        kind: "struct",
        fields: [
          { name: "oracle", type: "pubkey" },
          { name: "price", type: "u128" },
          { name: "cumulativePrice", type: "u128" },
          { name: "slot", type: "u64" },
          { name: "updater", type: "pubkey" },
        ],
      },
    },
    {
      name: "ownershipTransferred",
      type: {
        kind: "struct",
        fields: [
          { name: "oracle", type: "pubkey" },
          { name: "previousOwner", type: "pubkey" },
          { name: "newOwner", type: "pubkey" },
        ],
      },
    },
    {
      name: "oraclePauseToggled",
      type: {
        kind: "struct",
        fields: [
          { name: "oracle", type: "pubkey" },
          { name: "paused", type: "bool" },
        ],
      },
    },
  ],
};
