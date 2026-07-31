using System;
using System.Collections.Generic;

using Shop.Domain.Billing;
using Shop.Data;

namespace Shop.Domain.Orders;

public sealed class Order
{
    public Guid Id { get; init; }
    public IReadOnlyList<string> Lines { get; init; } = new List<string>();

    public decimal Total() => Invoice.For(this);

    public static Order Load(Guid id) => OrderStore.Fetch(id);
}
