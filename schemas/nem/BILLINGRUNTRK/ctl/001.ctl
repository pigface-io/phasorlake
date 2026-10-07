-- ************************************************************ 
-- Title:	PUBLIC_DVD_BILLINGRUNTRK_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BILLINGRUNTRK owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BILLINGRUNTRK_201501010000.csv  
APPEND INTO TABLE BILLINGRUNTRK  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
CONTRACTYEAR FLOAT EXTERNAL, 
WEEKNO FLOAT EXTERNAL, 
BILLRUNNO FLOAT EXTERNAL, 
STATUS CHAR(6), 
ADJ_CLEARED CHAR(1), 
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
AUTHORISEDBY CHAR(10), 
POSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
POSTBY CHAR(10), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
RECEIPTPOSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
RECEIPTPOSTBY CHAR(10), 
PAYMENTPOSTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
PAYMENTPOSTBY CHAR(10), 
SHORTFALL FLOAT EXTERNAL, 
MAKEUP FLOAT EXTERNAL) 
-- End of Script 
