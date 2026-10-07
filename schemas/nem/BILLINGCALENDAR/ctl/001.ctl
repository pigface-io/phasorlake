-- ************************************************************ 
-- Title:	PUBLIC_DVD_BILLINGCALENDAR_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BILLINGCALENDAR owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BILLINGCALENDAR_201501010000.csv  
APPEND INTO TABLE BILLINGCALENDAR	
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
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
PRELIMINARYSTATEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
FINALSTATEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
PAYMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
REVISION1_STATEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
REVISION2_STATEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
