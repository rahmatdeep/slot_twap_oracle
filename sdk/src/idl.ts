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
      args: [{ name: "windowSlots"; type: "u64" }];
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
        {
          name: "oracle";
          writable: true;
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
        }
      ];
      args: [{ name: "newPrice"; type: "u128" }];
    }
  ];
  accounts: [
    { name: "observationBuffer"; discriminator: [251, 96, 31, 90, 232, 132, 250, 134] },
    { name: "oracle"; discriminator: [139, 194, 131, 179, 140, 179, 229, 244] }
  ];
  events: [
    { name: "priceUpdated"; discriminator: [154, 72, 87, 150, 246, 230, 23, 217] },
    { name: "updateSubmitted"; discriminator: [210, 175, 191, 96, 27, 89, 235, 139] }
  ];
  errors: [
    { code: 6000; name: "priceOverflow"; msg: "Price overflow detected" },
    { code: 6001; name: "staleSlot"; msg: "Stale oracle update — slot has not advanced" },
    { code: 6002; name: "insufficientHistory"; msg: "Not enough observations to compute swap for requested window" },
    { code: 6003; name: "invalidCapacity"; msg: "Observation buffer capacity must be greater than zero" }
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
          { name: "baseMint"; type: "pubkey" },
          { name: "quoteMint"; type: "pubkey" },
          { name: "lastPrice"; type: "u128" },
          { name: "cumulativePrice"; type: "u128" },
          { name: "lastSlot"; type: "u64" },
          { name: "lastUpdater"; type: "pubkey" }
        ];
      };
    },
    {
      name: "priceUpdated";
      type: {
        kind: "struct";
        fields: [
          { name: "slot"; type: "u64" },
          { name: "newPrice"; type: "u128" },
          { name: "cumulativePrice"; type: "u128" }
        ];
      };
    },
    {
      name: "updateSubmitted";
      type: {
        kind: "struct";
        fields: [
          { name: "updater"; type: "pubkey" },
          { name: "slot"; type: "u64" },
          { name: "price"; type: "u128" }
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
      args: [{ name: "windowSlots", type: "u64" }],
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
        {
          name: "oracle",
          writable: true,
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
      ],
      args: [{ name: "newPrice", type: "u128" }],
    },
  ],
  accounts: [
    { name: "observationBuffer", discriminator: [251, 96, 31, 90, 232, 132, 250, 134] },
    { name: "oracle", discriminator: [139, 194, 131, 179, 140, 179, 229, 244] },
  ],
  events: [{ name: "priceUpdated", discriminator: [154, 72, 87, 150, 246, 230, 23, 217] }],
  errors: [
    { code: 6000, name: "priceOverflow", msg: "Price overflow detected" },
    { code: 6001, name: "staleSlot", msg: "Stale oracle update — slot has not advanced" },
    { code: 6002, name: "insufficientHistory", msg: "Not enough observations to compute swap for requested window" },
    { code: 6003, name: "invalidCapacity", msg: "Observation buffer capacity must be greater than zero" },
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
          { name: "baseMint", type: "pubkey" },
          { name: "quoteMint", type: "pubkey" },
          { name: "lastPrice", type: "u128" },
          { name: "cumulativePrice", type: "u128" },
          { name: "lastSlot", type: "u64" },
        ],
      },
    },
    {
      name: "priceUpdated",
      type: {
        kind: "struct",
        fields: [
          { name: "slot", type: "u64" },
          { name: "newPrice", type: "u128" },
          { name: "cumulativePrice", type: "u128" },
        ],
      },
    },
  ],
};
