-- ************************************************************ 
-- Title:	PUBLIC_DVD_INTERCONNECTOR_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS INTERCONNECTOR owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_INTERCONNECTOR_201501010000.csv  
APPEND INTO TABLE INTERCONNECTOR  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
INTERCONNECTORID CHAR(10), 
REGIONFROM CHAR(10), 
RSOID CHAR(10), 
REGIONTO CHAR(10), 
DESCRIPTION CHAR(64), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
