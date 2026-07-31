global using System;

using Serilog;

using Shop.Data;
using Shop.Domain.Orders;

namespace Shop.Api;

public sealed class OrderController
{
    public decimal Show(Guid id)
    {
        Log.Information("fetching {Id}", id);
        // Reaching straight into the data layer instead of going through the domain.
        var order = OrderStore.Fetch(id);
        return order.Total();
    }
}
