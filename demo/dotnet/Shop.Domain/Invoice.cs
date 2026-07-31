using System;

using Shop.Domain.Orders;

namespace Shop.Domain.Billing;

public static class Invoice
{
    public static decimal For(Order order) => order.Lines.Count * 9.99m;
}
