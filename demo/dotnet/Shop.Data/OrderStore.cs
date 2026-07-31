using System;

using Dapper;

using Shop.Domain.Orders;

namespace Shop.Data;

public static class OrderStore
{
    public static Order Fetch(Guid id)
    {
        _ = new CommandDefinition("select 1");
        return new Order { Id = id };
    }
}
